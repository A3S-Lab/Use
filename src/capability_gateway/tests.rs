use std::sync::Mutex;
use std::time::Duration;

use a3s_use_core::{
    ArtifactRef, CapabilityDescriptor, CapabilityDescriptorKind, CapabilityGatewayCatalog,
    CapabilityPublicationEvidence, CapabilityToolAnnotations, EndpointRef, InstallationId,
    InstallationKind, InvocationRef, PluginPackageId, PluginSurfaceKind, PluginSurfaceRef,
    CAPABILITY_DESCRIPTOR_SCHEMA_V1,
};
use rmcp::model::CallToolRequestParam;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClientTransport, StreamableHttpClientTransportConfig,
};
use rmcp::{ClientHandler, ServiceExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio_util::sync::CancellationToken;

use crate::capability_registry::{CapabilityPackageGeneration, CAPABILITY_SNAPSHOT_CURSOR_SCHEMA};

use super::*;

#[derive(Debug, Default)]
struct RecordingProvider {
    calls: Mutex<Vec<(InvocationRef, Value)>>,
}

#[async_trait]
impl CapabilityGatewayInvocationProvider for RecordingProvider {
    async fn authorize(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: &Value,
    ) -> UseResult<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: Value,
    ) -> UseResult<Value> {
        self.calls
            .lock()
            .map_err(|_| UseError::new("test.provider_poisoned", "Provider lock poisoned."))?
            .push((descriptor.invocation_ref.clone(), arguments.clone()));
        Ok(serde_json::json!({ "ok": true, "arguments": arguments }))
    }
}

#[derive(Debug, Default)]
struct InvalidOutputProvider;

#[async_trait]
impl CapabilityGatewayInvocationProvider for InvalidOutputProvider {
    async fn authorize(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: &Value,
    ) -> UseResult<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: Value,
    ) -> UseResult<Value> {
        Ok(serde_json::json!({
            "ok": "not-a-boolean",
            "arguments": {}
        }))
    }
}

#[derive(Debug, Default)]
struct FailingProvider;

#[async_trait]
impl CapabilityGatewayInvocationProvider for FailingProvider {
    async fn authorize(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: &Value,
    ) -> UseResult<()> {
        Ok(())
    }

    async fn invoke(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: Value,
    ) -> UseResult<Value> {
        Err(UseError::new(
            "provider.internal_secret",
            "private token /srv/a3s/secrets/token",
        )
        .with_suggestion("read /srv/a3s/secrets/token")
        .with_detail("path", "/srv/a3s/secrets/token"))
    }
}

#[derive(Debug, Default)]
struct DenyingProvider {
    calls: Mutex<Vec<InvocationRef>>,
}

#[async_trait]
impl CapabilityGatewayInvocationProvider for DenyingProvider {
    async fn authorize(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: &Value,
    ) -> UseResult<()> {
        Err(UseError::new(
            "host.private_authorization_reason",
            "private policy details must not cross the Gateway boundary",
        ))
    }

    async fn invoke(
        &self,
        descriptor: &CapabilityDescriptor,
        _arguments: Value,
    ) -> UseResult<Value> {
        self.calls
            .lock()
            .map_err(|_| UseError::new("test.provider_poisoned", "Provider lock poisoned."))?
            .push(descriptor.invocation_ref.clone());
        Ok(serde_json::json!({ "ok": true }))
    }
}

#[derive(Debug, Clone, Copy, Default)]
struct TestClient;

impl ClientHandler for TestClient {}

#[test]
fn gateway_admission_is_bounded_and_shared() {
    let limits = CapabilityGatewayLimits::new(1, 2, Duration::from_secs(60)).unwrap();
    let admission = GatewayAdmission::new(limits).unwrap();
    let permit = admission.try_acquire().expect("first call is admitted");
    assert_eq!(
        admission.try_acquire().unwrap_err(),
        AdmissionFailure::InFlight
    );
    drop(permit);
    let permit = admission.try_acquire().expect("permit is released");
    drop(permit);
    assert_eq!(
        admission.try_acquire().unwrap_err(),
        AdmissionFailure::RateLimited
    );
}

#[test]
fn gateway_limits_and_http_credentials_fail_closed() {
    assert!(CapabilityGatewayLimits::new(0, 1, Duration::from_secs(1)).is_err());
    assert!(CapabilityGatewayLimits::new(1, 1, Duration::ZERO).is_err());
    assert!(CapabilityGatewayLimits::new(1, 1, Duration::from_secs(60 * 60 + 1)).is_err());
    assert!(CapabilityGatewayHttpConfig::new("bad token").is_err());
    assert!(CapabilityGatewayHttpConfig::new("bad\n-token").is_err());

    let config = CapabilityGatewayHttpConfig::new("secret-token")
        .unwrap()
        .with_allowed_origin("https://agent.example")
        .unwrap();
    let debug = format!("{config:?}");
    assert!(debug.contains("[redacted]"));
    assert!(!debug.contains("secret-token"));
    assert!(CapabilityGatewayHttpConfig::new("secret-token")
        .unwrap()
        .with_allowed_origin("https://agent example")
        .is_err());
}

#[tokio::test]
async fn adapter_uses_standard_mcp_initialization_list_and_call() {
    let descriptor = test_descriptor();
    let invocation_ref = descriptor.invocation_ref.clone();
    let catalog = test_catalog(descriptor);
    let provider = Arc::new(RecordingProvider::default());
    let server = CapabilityGatewayMcpServer::new(catalog, provider.clone()).unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = TestClient.serve(client_transport).await.unwrap();

    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "search");
    assert_eq!(tools[0].title.as_deref(), Some("Search"));
    assert!(tools[0].output_schema.is_some());
    assert_eq!(
        tools[0].annotations.as_ref().unwrap().read_only_hint,
        Some(true)
    );
    let advertised = serde_json::to_value(&tools[0]).unwrap();
    for private_field in [
        "invocationRef",
        "artifactRef",
        "endpointRef",
        "packageRoot",
        "path",
        "secret",
    ] {
        assert!(
            advertised.get(private_field).is_none(),
            "{private_field} leaked"
        );
    }

    let result = client
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                serde_json::json!({ "query": "mcp" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value["arguments"]["query"].as_str()),
        Some("mcp")
    );
    {
        let calls = provider.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, invocation_ref);
        assert_eq!(calls[0].1["query"], "mcp");
    }

    client.cancel().await.unwrap();
    server_handle.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamable_http_requires_explicit_auth_origin_and_limits_requests() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    let limits = CapabilityGatewayLimits::new(4, 1, Duration::from_secs(60)).unwrap();
    let config = CapabilityGatewayHttpConfig::new("secret-token")
        .unwrap()
        .with_allowed_origin("https://agent.example")
        .unwrap()
        .with_limits(limits)
        .unwrap();
    let server = CapabilityGatewayMcpServer::new(
        test_catalog(test_descriptor()),
        Arc::new(RecordingProvider::default()),
    )
    .unwrap();
    let server_handle =
        tokio::spawn(server.serve_streamable_http(listener, config, shutdown.clone()));
    tokio::time::sleep(Duration::from_millis(20)).await;

    let unauthorized = raw_gateway_http_response(port, "").await;
    assert!(unauthorized.starts_with("HTTP/1.1 401"));
    let duplicate_authorization = raw_gateway_http_response(
        port,
        "Authorization: Bearer secret-token\r\nAuthorization: Bearer secret-token\r\n",
    )
    .await;
    assert!(duplicate_authorization.starts_with("HTTP/1.1 401"));
    let untrusted_origin = raw_gateway_http_response(
        port,
        "Authorization: Bearer secret-token\r\nOrigin: https://attacker.example\r\n",
    )
    .await;
    assert!(untrusted_origin.starts_with("HTTP/1.1 403"));
    let duplicate_origin = raw_gateway_http_response(
        port,
        "Authorization: Bearer secret-token\r\nOrigin: https://agent.example\r\nOrigin: https://agent.example\r\n",
    )
    .await;
    assert!(duplicate_origin.starts_with("HTTP/1.1 403"));

    let authorized = raw_gateway_http_response(
        port,
        "Authorization: Bearer secret-token\r\nOrigin: https://agent.example\r\n",
    )
    .await;
    assert!(!authorized.starts_with("HTTP/1.1 401"));
    assert!(!authorized.starts_with("HTTP/1.1 403"));

    let limited = raw_gateway_http_response(
        port,
        "Authorization: Bearer secret-token\r\nOrigin: https://agent.example\r\n",
    )
    .await;
    assert!(limited.starts_with("HTTP/1.1 429"));

    shutdown.cancel();
    server_handle.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn streamable_http_accepts_a_standard_rust_client_without_shared_filesystem() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let shutdown = CancellationToken::new();
    let provider = Arc::new(RecordingProvider::default());
    let server =
        CapabilityGatewayMcpServer::new(test_catalog(test_descriptor()), provider.clone()).unwrap();
    let server_handle = tokio::spawn(server.serve_streamable_http(
        listener,
        CapabilityGatewayHttpConfig::new("secret-token").unwrap(),
        shutdown.clone(),
    ));
    tokio::time::sleep(Duration::from_millis(20)).await;

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(format!("http://127.0.0.1:{port}/mcp"))
            .auth_header("secret-token"),
    );
    let client = TestClient.serve(transport).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].name, "search");
    let result = client
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                serde_json::json!({ "query": "remote" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(false));
    assert_eq!(provider.calls.lock().unwrap().len(), 1);

    client.cancel().await.unwrap();
    shutdown.cancel();
    server_handle.await.unwrap().unwrap();
}

async fn raw_gateway_http_response(port: u16, headers: &str) -> String {
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    let request = format!(
        "GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n{headers}\r\n"
    );
    stream.write_all(request.as_bytes()).await.unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).await.unwrap();
    String::from_utf8(response).unwrap()
}

#[tokio::test]
async fn adapter_rejects_arguments_before_provider_dispatch() {
    let provider = Arc::new(RecordingProvider::default());
    let server =
        CapabilityGatewayMcpServer::new(test_catalog(test_descriptor()), provider.clone()).unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = TestClient.serve(client_transport).await.unwrap();

    let error = client
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                serde_json::json!({ "query": 42 })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .expect_err("invalid arguments must be rejected by the MCP boundary");
    assert!(error.to_string().contains("published schema"));
    assert!(provider.calls.lock().unwrap().is_empty());

    client.cancel().await.unwrap();
    server_handle.await.unwrap();
}

#[tokio::test]
async fn adapter_rejects_provider_output_outside_the_published_schema() {
    let server = CapabilityGatewayMcpServer::new(
        test_catalog(test_descriptor()),
        Arc::new(InvalidOutputProvider),
    )
    .unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = TestClient.serve(client_transport).await.unwrap();

    let result = client
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                serde_json::json!({ "query": "ok" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let error = result.structured_content.expect("structured MCP error");
    assert_eq!(error["code"], MCP_SCHEMA_ERROR);
    assert!(!error.to_string().contains("not-a-boolean"));

    client.cancel().await.unwrap();
    server_handle.await.unwrap();
}

#[tokio::test]
async fn adapter_sanitizes_provider_errors_at_the_agent_boundary() {
    let server =
        CapabilityGatewayMcpServer::new(test_catalog(test_descriptor()), Arc::new(FailingProvider))
            .unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = TestClient.serve(client_transport).await.unwrap();

    let result = client
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                serde_json::json!({ "query": "ok" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let error = result.structured_content.expect("structured MCP error");
    assert_eq!(error["code"], MCP_INVOCATION_ERROR);
    let serialized = error.to_string();
    assert!(!serialized.contains("private token"));
    assert!(!serialized.contains("/srv/a3s/secrets"));
    assert!(!serialized.contains("provider.internal_secret"));

    client.cancel().await.unwrap();
    server_handle.await.unwrap();
}

#[tokio::test]
async fn adapter_denies_unauthorized_calls_before_provider_dispatch() {
    let provider = Arc::new(DenyingProvider::default());
    let server =
        CapabilityGatewayMcpServer::new(test_catalog(test_descriptor()), provider.clone()).unwrap();
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let server_handle = tokio::spawn(async move {
        server
            .serve(server_transport)
            .await
            .unwrap()
            .waiting()
            .await
            .unwrap();
    });
    let client = TestClient.serve(client_transport).await.unwrap();

    let result = client
        .call_tool(CallToolRequestParam {
            name: "search".into(),
            arguments: Some(
                serde_json::json!({ "query": "secret" })
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        })
        .await
        .unwrap();
    assert_eq!(result.is_error, Some(true));
    let error = result
        .structured_content
        .expect("structured authorization error");
    assert_eq!(error["code"], MCP_AUTHORIZATION_ERROR);
    assert!(!error.to_string().contains("private policy"));
    assert!(provider.calls.lock().unwrap().is_empty());

    client.cancel().await.unwrap();
    server_handle.await.unwrap();
}

#[test]
fn adapter_rejects_a_schema_the_fixed_validator_cannot_compile() {
    let mut descriptor = test_descriptor();
    if let CapabilityDescriptorKind::Tool { input_schema, .. } = &mut descriptor.capability {
        *input_schema = serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {"query": {"type": "not-a-json-type"}}
        });
    }
    let error = CapabilityGatewayMcpServer::new(
        test_catalog(descriptor),
        Arc::new(RecordingProvider::default()),
    )
    .expect_err("invalid schemas must not become callable routes");
    assert_eq!(error.code, MCP_ERROR);
    assert!(!error.message.contains("not-a-json-type"));
}

#[test]
fn compiled_schemas_never_resolve_external_resources() {
    let error = CompiledCapabilitySchema::compile(&serde_json::json!({
        "$ref": "https://example.invalid/schema"
    }))
    .expect_err("external schema resources must be rejected");
    assert_eq!(error.code, MCP_ERROR);
    assert!(!error.message.contains("example.invalid"));
}

#[test]
fn adapter_exposes_only_agent_tools_and_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityGatewayMcpServer>();
    assert_send_sync::<RecordingProvider>();

    let catalog = test_catalog(test_descriptor());
    let provider = Arc::new(RecordingProvider::default());
    let server = CapabilityGatewayMcpServer::new(catalog, provider).unwrap();
    assert_eq!(server.catalog().descriptors().len(), 1);
    assert_eq!(server.tool_router.list_all().len(), 1);
    assert!(server.snapshot_cursor().is_none());
}

#[test]
fn snapshot_binding_requires_exact_package_generation_evidence() {
    let descriptor = test_descriptor();
    let installation = InstallationId::new(InstallationKind::User, "user/gateway-tests").unwrap();
    let cursor = test_cursor(&descriptor, installation.clone());
    let catalog = CapabilityGatewayCatalog::new(
        installation.clone(),
        cursor.generation,
        vec![descriptor.clone()],
    )
    .unwrap();

    validate_snapshot_binding_identity(&catalog, installation.clone(), cursor.generation, &cursor)
        .unwrap();

    let lifecycle_bound_catalog = CapabilityGatewayCatalog::new(
        installation.clone(),
        descriptor.generation,
        vec![descriptor.clone()],
    )
    .unwrap();
    assert_eq!(
        validate_snapshot_binding_identity(
            &lifecycle_bound_catalog,
            installation.clone(),
            cursor.generation,
            &cursor,
        )
        .unwrap_err()
        .code,
        MCP_ERROR
    );

    let mut wrong_generation = cursor.clone();
    wrong_generation.generation += 1;
    assert_eq!(
        validate_snapshot_binding_identity(
            &catalog,
            installation.clone(),
            cursor.generation,
            &wrong_generation,
        )
        .unwrap_err()
        .code,
        MCP_ERROR
    );

    let mut wrong_digest = cursor.clone();
    wrong_digest.packages[0].package_digest = format!("sha256:{}", "f".repeat(64));
    assert_eq!(
        validate_snapshot_binding_identity(
            &catalog,
            installation.clone(),
            cursor.generation,
            &wrong_digest,
        )
        .unwrap_err()
        .code,
        MCP_ERROR
    );

    let mut missing_package = cursor.clone();
    missing_package.packages.clear();
    assert_eq!(
        validate_snapshot_binding_identity(
            &catalog,
            installation,
            cursor.generation,
            &missing_package,
        )
        .unwrap_err()
        .code,
        MCP_ERROR
    );
}

#[test]
fn snapshot_binding_rejects_unleasable_packages() {
    let descriptor = test_descriptor();
    let catalog = test_catalog(descriptor.clone());
    let installation = catalog.installation().clone();
    let mut cursor = test_cursor(&descriptor, installation.clone());
    cursor.unleasable_packages = vec!["acme/other".to_owned()];
    assert_eq!(
        validate_snapshot_binding_identity(&catalog, installation, cursor.generation, &cursor,)
            .unwrap_err()
            .code,
        MCP_ERROR
    );
}

#[cfg(feature = "extensions")]
#[test]
fn leased_server_retains_the_exact_snapshot_across_clones() {
    std::thread::Builder::new()
        .name("capability-gateway-lease".to_owned())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(leased_server_snapshot_lifetime());
        })
        .unwrap()
        .join()
        .unwrap();
}

#[cfg(feature = "extensions")]
async fn leased_server_snapshot_lifetime() {
    let temporary = tempfile::tempdir().unwrap();
    let installation =
        InstallationId::new(InstallationKind::Workspace, "gateway-lease-tests").unwrap();
    let registry = a3s_use_extension::ExtensionRegistry::new(
        a3s_use_extension::ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            installation.clone(),
        )
        .unwrap(),
    );
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-cognitive/package");
    let package = a3s_use_extension::ExtensionLifecyclePackage::prepare_local(
        "acme/cognitive",
        &fixture,
        true,
    )
    .await
    .unwrap();
    let identity = a3s_use_extension::ExtensionLifecycleIdentity::new(
        package.package_id(),
        package.package_digest(),
        package.manifest_digest(),
        1,
    )
    .unwrap();
    registry
        .commit_lifecycle_package(&identity, &package)
        .await
        .unwrap();
    registry.publish_lifecycle_package(&identity).await.unwrap();

    let capability_registry = crate::capability_registry::CapabilityRegistry::new(registry);
    let snapshot = capability_registry.snapshot().await.unwrap();
    let package = &snapshot.cursor().packages[0];
    let package_id = package.package_id.clone();
    let lifecycle_generation = package.lifecycle_generation;
    let package_digest = package.package_digest.clone();
    let manifest_digest = package.manifest_digest.clone();
    let mut descriptor = test_descriptor();
    descriptor.package_id = PluginPackageId::parse(package_id).unwrap();
    descriptor.generation = lifecycle_generation;
    descriptor.package_digest = package_digest;
    descriptor.manifest_digest = manifest_digest;
    descriptor.invocation_ref = InvocationRef::derive(
        &descriptor.package_id,
        &descriptor.surface,
        descriptor.generation,
        &format!("sha256:{}", "1".repeat(64)),
    )
    .unwrap();
    descriptor.artifact_ref = Some(
        ArtifactRef::derive(
            &descriptor.package_id,
            &descriptor.surface,
            descriptor.generation,
            &format!("sha256:{}", "2".repeat(64)),
        )
        .unwrap(),
    );
    descriptor.endpoint_ref = Some(
        EndpointRef::derive(
            &descriptor.package_id,
            &descriptor.surface,
            descriptor.generation,
            &format!("sha256:{}", "3".repeat(64)),
        )
        .unwrap(),
    );
    let catalog =
        CapabilityGatewayCatalog::new(installation, snapshot.generation, vec![descriptor]).unwrap();
    let server = CapabilityGatewayMcpServer::from_registry(
        &capability_registry,
        catalog,
        Arc::new(RecordingProvider::default()),
    )
    .await
    .unwrap()
    .expect("published package must be leasable");
    let cursor = server.snapshot_cursor().cloned().unwrap();
    let clone = server.clone();
    drop(server);
    assert_eq!(clone.snapshot_cursor(), Some(&cursor));
}

fn test_catalog(descriptor: CapabilityDescriptor) -> CapabilityGatewayCatalog {
    CapabilityGatewayCatalog::new(
        InstallationId::new(InstallationKind::User, "user/gateway-tests").unwrap(),
        9,
        vec![descriptor],
    )
    .unwrap()
}

fn test_cursor(
    descriptor: &CapabilityDescriptor,
    installation: InstallationId,
) -> CapabilitySnapshotCursor {
    CapabilitySnapshotCursor {
        schema: CAPABILITY_SNAPSHOT_CURSOR_SCHEMA.to_owned(),
        installation,
        installation_generation: None,
        installation_snapshot_digest: None,
        // The Registry publication generation is independent from the
        // package lifecycle generation carried by each descriptor.
        generation: 9,
        revision: "a".repeat(64),
        registry_revision: format!("sha256:{}", "b".repeat(64)),
        packages: vec![CapabilityPackageGeneration {
            package_id: descriptor.package_id.to_string(),
            lifecycle_generation: descriptor.generation,
            package_digest: descriptor.package_digest.clone(),
            manifest_digest: descriptor.manifest_digest.clone(),
        }],
        unleasable_packages: Vec::new(),
    }
}

fn test_descriptor() -> CapabilityDescriptor {
    let package_id = PluginPackageId::parse("acme/assistant").unwrap();
    let surface = PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "search".to_owned(),
    };
    let digest = |letter: char| format!("sha256:{}", letter.to_string().repeat(64));
    let schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": { "query": { "type": "string" } },
        "required": ["query"]
    });
    let output_schema = serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "arguments": schema.clone(),
            "ok": { "type": "boolean" }
        },
        "required": ["arguments", "ok"]
    });
    CapabilityDescriptor {
        schema: CAPABILITY_DESCRIPTOR_SCHEMA_V1.to_owned(),
        package_id: package_id.clone(),
        surface: surface.clone(),
        generation: 1,
        package_digest: digest('a'),
        manifest_digest: digest('b'),
        title: "Search".to_owned(),
        description: "Search verified knowledge.".to_owned(),
        invocation_ref: InvocationRef::derive(&package_id, &surface, 1, &digest('c')).unwrap(),
        artifact_ref: Some(ArtifactRef::derive(&package_id, &surface, 1, &digest('d')).unwrap()),
        endpoint_ref: Some(EndpointRef::derive(&package_id, &surface, 1, &digest('e')).unwrap()),
        dependencies: Vec::new(),
        publication: CapabilityPublicationEvidence {
            catalog_record_digest: digest('f'),
            signature_digest: digest('0'),
        },
        capability: CapabilityDescriptorKind::Tool {
            name: "search".to_owned(),
            input_schema: schema.clone(),
            output_schema,
            annotations: CapabilityToolAnnotations::new(true, false, true, false),
        },
    }
}
