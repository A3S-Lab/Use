//! Standard MCP adapter for the path-free Capability Gateway contract.
//!
//! The adapter is intentionally a thin protocol boundary.  It owns an
//! immutable, generation-bound catalog and delegates every invocation to an
//! injected provider.  The provider resolves the opaque invocation reference
//! inside the host authority; no client-supplied path, package root, endpoint,
//! or credential is accepted by this module.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{
    CapabilityDescriptor, CapabilityDescriptorKind, CapabilityGatewayCatalog,
    CapabilityToolAnnotations, UseError, UseResult,
};
use async_trait::async_trait;
use jsonschema::{Draft, Validator};
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::model::{
    CallToolResult, Implementation, JsonObject, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::{tool_handler, ServerHandler, ServiceExt};
use serde_json::Value;

const MCP_ERROR: &str = "use.plugin.capability_gateway_mcp_invalid";
const MCP_SCHEMA_ERROR: &str = "use.plugin.capability_gateway_schema_violation";
const MCP_INVOCATION_ERROR: &str = "use.plugin.capability_gateway_invocation_failed";
const MAX_CAPABILITY_VALUE_BYTES: usize = 256 * 1024;
const MAX_CAPABILITY_VALUE_DEPTH: usize = 32;
const MAX_CAPABILITY_VALUE_ELEMENTS: usize = 4_096;

#[derive(Clone)]
struct CompiledCapabilitySchema {
    validator: Arc<Validator>,
}

impl std::fmt::Debug for CompiledCapabilitySchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompiledCapabilitySchema")
            .finish_non_exhaustive()
    }
}

impl CompiledCapabilitySchema {
    fn compile(schema: &Value) -> UseResult<Self> {
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_retriever(NoExternalSchemaRetriever)
            .build(schema)
            .map_err(|_| {
                mcp_error(
                    "The Capability Gateway schema cannot be compiled by the fixed validator.",
                )
            })?;
        Ok(Self {
            validator: Arc::new(validator),
        })
    }

    fn validate(&self, value: &Value) -> UseResult<()> {
        let encoded = serde_json::to_vec(value).map_err(|_| schema_value_error())?;
        if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
            return Err(schema_value_error());
        }
        validate_value_bounds(value, 0)?;
        if self.validator.is_valid(value) {
            Ok(())
        } else {
            Err(schema_value_error())
        }
    }
}

/// Capability schemas are self-contained contract data.  An agent-visible
/// descriptor must never make validator construction read a URL or local
/// file, even if a future schema keyword introduces another reference form.
#[derive(Debug, Clone, Copy)]
struct NoExternalSchemaRetriever;

impl jsonschema::Retrieve for NoExternalSchemaRetriever {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external Capability Gateway schema retrieval is disabled".into())
    }
}

#[derive(Clone)]
struct CapabilityGatewayTool {
    descriptor_index: usize,
    input_schema: CompiledCapabilitySchema,
    output_schema: CompiledCapabilitySchema,
}

/// Host-owned invocation boundary for a Capability Gateway Tool.
///
/// Implementations must resolve `descriptor.invocation_ref` against their
/// private, generation-fenced authority.  The `arguments` value contains only
/// the MCP tool arguments; it never contains an invocation or endpoint
/// reference supplied by the client.
#[async_trait]
pub trait CapabilityGatewayInvocationProvider: Send + Sync {
    async fn invoke(&self, descriptor: &CapabilityDescriptor, arguments: Value)
        -> UseResult<Value>;
}

/// Standard MCP server backed by an immutable Capability Gateway catalog.
#[derive(Clone)]
pub struct CapabilityGatewayMcpServer {
    catalog: Arc<CapabilityGatewayCatalog>,
    provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    tools: Arc<BTreeMap<String, CapabilityGatewayTool>>,
    tool_router: ToolRouter<Self>,
}

impl std::fmt::Debug for CapabilityGatewayMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayMcpServer")
            .field("catalog", &self.catalog)
            .finish_non_exhaustive()
    }
}

impl CapabilityGatewayMcpServer {
    /// Compose an MCP adapter and freeze the catalog for its lifetime.
    pub fn new(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    ) -> UseResult<Self> {
        catalog.validate()?;
        let catalog = Arc::new(catalog);
        let tools = Arc::new(compile_tools(&catalog)?);
        let tool_router = frozen_tool_router(&catalog)?;
        Ok(Self {
            catalog,
            provider,
            tools,
            tool_router,
        })
    }

    /// Return the exact immutable catalog used by this server.
    pub fn catalog(&self) -> &CapabilityGatewayCatalog {
        &self.catalog
    }

    /// Serve standard MCP framing over stdin/stdout until the peer
    /// disconnects.  No A3S-specific JSON-RPC dialect is introduced.
    pub async fn serve_stdio(self) -> UseResult<()> {
        let service = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|_| mcp_error("Failed to start Capability Gateway MCP."))?;
        service
            .waiting()
            .await
            .map_err(|_| mcp_error("Capability Gateway MCP stopped with an error."))?;
        Ok(())
    }

    async fn dispatch(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tool = self.tools.get(name).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "Capability Gateway Tool is not part of the immutable catalog.",
                None,
            )
        })?;
        let descriptor = self
            .catalog
            .descriptors()
            .get(tool.descriptor_index)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "Capability Gateway route index is inconsistent with its catalog.",
                    None,
                )
            })?;
        let arguments = Value::Object(arguments.unwrap_or_default());
        tool.input_schema.validate(&arguments).map_err(|_| {
            rmcp::ErrorData::invalid_params(
                "Capability Gateway Tool arguments do not satisfy the published schema.",
                None,
            )
        })?;
        let result = self.provider.invoke(descriptor, arguments).await;
        Ok(tool_result(result, &tool.output_schema))
    }
}

#[tool_handler]
impl ServerHandler for CapabilityGatewayMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "a3s-use-capability-gateway".to_owned(),
                title: Some("A3S Use Capability Gateway".to_owned()),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_owned()),
            },
            instructions: Some(
                "The catalog is immutable and generation-bound. Tool calls are resolved by the host from opaque references; client-supplied paths, package roots, endpoints, and credentials are not accepted."
                    .to_owned(),
            ),
            ..Default::default()
        }
    }
}

fn frozen_tool_router(
    catalog: &Arc<CapabilityGatewayCatalog>,
) -> UseResult<ToolRouter<CapabilityGatewayMcpServer>> {
    let mut router = ToolRouter::<CapabilityGatewayMcpServer>::new();
    for descriptor in catalog
        .descriptors()
        .iter()
        .filter(|descriptor| descriptor.is_agent_tool())
    {
        let name = descriptor.tool_name().ok_or_else(|| {
            mcp_error("The Capability Gateway catalog contains a non-Tool route.")
        })?;
        if router.has_route(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate Tool name `{name}`."
            )));
        }
        let tool = mcp_tool(descriptor)?;
        let route_name = name.to_owned();
        router.add_route(ToolRoute::new_dyn(
            tool,
            move |context: rmcp::handler::server::tool::ToolCallContext<
                '_,
                CapabilityGatewayMcpServer,
            >| {
                let route_name = route_name.clone();
                Box::pin(async move {
                    context
                        .service
                        .dispatch(&route_name, context.arguments)
                        .await
                })
            },
        ));
    }
    Ok(router)
}

fn compile_tools(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayTool>> {
    let mut tools = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        if !descriptor.is_agent_tool() {
            continue;
        }
        let name = descriptor.tool_name().ok_or_else(|| {
            mcp_error("The Capability Gateway catalog contains a non-Tool route.")
        })?;
        if tools.contains_key(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate Tool name `{name}`."
            )));
        }
        let (input_schema, output_schema) = match &descriptor.capability {
            CapabilityDescriptorKind::Tool {
                input_schema,
                output_schema,
                ..
            } => (input_schema, output_schema),
            CapabilityDescriptorKind::McpServer { .. } => {
                return Err(mcp_error(
                    "Only Tool descriptors can be compiled for the MCP Gateway.",
                ));
            }
        };
        tools.insert(
            name.to_owned(),
            CapabilityGatewayTool {
                descriptor_index,
                input_schema: CompiledCapabilitySchema::compile(input_schema)?,
                output_schema: CompiledCapabilitySchema::compile(output_schema)?,
            },
        );
    }
    Ok(tools)
}

fn mcp_tool(descriptor: &CapabilityDescriptor) -> UseResult<Tool> {
    let CapabilityDescriptorKind::Tool {
        name,
        input_schema,
        output_schema,
        annotations,
    } = &descriptor.capability
    else {
        return Err(mcp_error("Only Tool descriptors can be exposed over MCP."));
    };
    let input_schema = input_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A Capability Gateway input schema is not an object."))?;
    let output_schema = output_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A Capability Gateway output schema is not an object."))?;
    Ok(Tool {
        name: Cow::Owned(name.clone()),
        title: Some(descriptor.title.clone()),
        description: Some(Cow::Owned(descriptor.description.clone())),
        input_schema: Arc::new(input_schema),
        output_schema: Some(Arc::new(output_schema)),
        annotations: Some(mcp_annotations(*annotations)),
        icons: None,
    })
}

fn mcp_annotations(annotations: CapabilityToolAnnotations) -> ToolAnnotations {
    ToolAnnotations {
        title: None,
        read_only_hint: Some(annotations.read_only_hint),
        destructive_hint: Some(annotations.destructive_hint),
        idempotent_hint: Some(annotations.idempotent_hint),
        open_world_hint: Some(annotations.open_world_hint),
    }
}

fn tool_result(
    result: UseResult<Value>,
    output_schema: &CompiledCapabilitySchema,
) -> CallToolResult {
    match result {
        Ok(value) => match output_schema.validate(&value) {
            Ok(()) => CallToolResult::structured(value),
            Err(_) => structured_error(
                MCP_SCHEMA_ERROR,
                "The Capability Gateway provider returned a value outside the published schema.",
            ),
        },
        Err(_) => structured_error(
            MCP_INVOCATION_ERROR,
            "The Capability Gateway provider could not complete the invocation.",
        ),
    }
}

fn structured_error(code: &str, message: &str) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "code": code,
        "message": message,
    }))
}

fn schema_value_error() -> UseError {
    UseError::new(
        MCP_SCHEMA_ERROR,
        "The capability value does not satisfy its published schema.",
    )
}

fn validate_value_bounds(value: &Value, depth: usize) -> UseResult<()> {
    if depth > MAX_CAPABILITY_VALUE_DEPTH {
        return Err(schema_value_error());
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
                return Err(schema_value_error());
            }
            for value in values {
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Object(object) => {
            if object.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
                return Err(schema_value_error());
            }
            for (key, value) in object {
                if key.len() > 4 * 1024 || key.chars().any(char::is_control) {
                    return Err(schema_value_error());
                }
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn mcp_error(message: impl Into<String>) -> UseError {
    UseError::new(MCP_ERROR, message)
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use a3s_use_core::{
        ArtifactRef, CapabilityDescriptor, CapabilityDescriptorKind, CapabilityGatewayCatalog,
        CapabilityPublicationEvidence, CapabilityToolAnnotations, EndpointRef, InstallationId,
        InstallationKind, InvocationRef, PluginPackageId, PluginSurfaceKind, PluginSurfaceRef,
        CAPABILITY_DESCRIPTOR_SCHEMA_V1,
    };
    use rmcp::model::CallToolRequestParam;
    use rmcp::{ClientHandler, ServiceExt};

    use super::*;

    #[derive(Debug, Default)]
    struct RecordingProvider {
        calls: Mutex<Vec<(InvocationRef, Value)>>,
    }

    #[async_trait]
    impl CapabilityGatewayInvocationProvider for RecordingProvider {
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

    #[derive(Debug, Clone, Copy, Default)]
    struct TestClient;

    impl ClientHandler for TestClient {}

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

    #[tokio::test]
    async fn adapter_rejects_arguments_before_provider_dispatch() {
        let provider = Arc::new(RecordingProvider::default());
        let server =
            CapabilityGatewayMcpServer::new(test_catalog(test_descriptor()), provider.clone())
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
        let server = CapabilityGatewayMcpServer::new(
            test_catalog(test_descriptor()),
            Arc::new(FailingProvider),
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
        assert_eq!(error["code"], MCP_INVOCATION_ERROR);
        let serialized = error.to_string();
        assert!(!serialized.contains("private token"));
        assert!(!serialized.contains("/srv/a3s/secrets"));
        assert!(!serialized.contains("provider.internal_secret"));

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
    }

    fn test_catalog(descriptor: CapabilityDescriptor) -> CapabilityGatewayCatalog {
        CapabilityGatewayCatalog::new(
            InstallationId::new(InstallationKind::User, "user/gateway-tests").unwrap(),
            descriptor.generation,
            vec![descriptor],
        )
        .unwrap()
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
            artifact_ref: Some(
                ArtifactRef::derive(&package_id, &surface, 1, &digest('d')).unwrap(),
            ),
            endpoint_ref: Some(
                EndpointRef::derive(&package_id, &surface, 1, &digest('e')).unwrap(),
            ),
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
}
