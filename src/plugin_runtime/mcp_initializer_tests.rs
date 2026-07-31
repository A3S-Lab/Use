use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::PluginSurfaceKind;
use axum::extract::Request;
use axum::http::header::AUTHORIZATION;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::model::{Implementation, ProtocolVersion, ServerCapabilities, ServerInfo};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::ServerHandler;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use url::Url;

use super::test_support::*;
use super::*;

const TOKEN: &str = "test-gateway-token";

#[tokio::test]
async fn real_mcp_initialize_negotiates_protocol_and_builds_a_binding_receipt() {
    let host = TestMcpHost::spawn("2025-06-18").await;
    let (activation, endpoint_ref) = activation().await;
    let connection = connection(&activation, &endpoint_ref, host.endpoint.clone());
    let initializer = RuntimeMcpInitializer::new(3_000).unwrap();

    let initialize = initializer
        .initialize(&activation, &endpoint_ref, &connection)
        .await
        .unwrap();
    let receipt = activation
        .into_mcp_service_receipt(endpoint_ref, initialize)
        .unwrap();

    assert!(matches!(
        receipt.readiness,
        RuntimeServiceReadinessEvidence::McpInitialized { .. }
    ));
    let RuntimeSurfaceContract::McpService {
        protocol_version, ..
    } = receipt.contract
    else {
        panic!("the receipt must retain its MCP Service contract");
    };
    assert_eq!(protocol_version, "2025-06-18");
    host.shutdown().await;
}

#[tokio::test]
async fn negotiated_protocol_downgrade_fails_closed() {
    let host = TestMcpHost::spawn("2025-03-26").await;
    let (activation, endpoint_ref) = activation().await;
    let connection = connection(&activation, &endpoint_ref, host.endpoint.clone());
    let initializer = RuntimeMcpInitializer::new(3_000).unwrap();

    let error = initializer
        .initialize(&activation, &endpoint_ref, &connection)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.mcp_protocol_mismatch");
    host.shutdown().await;
}

#[tokio::test]
async fn mcp_initialize_timeout_is_bounded() {
    let host = HangingHttpHost::spawn().await;
    let (activation, endpoint_ref) = activation().await;
    let connection = connection(&activation, &endpoint_ref, host.endpoint.clone());
    let initializer = RuntimeMcpInitializer::new(25).unwrap();

    let error = initializer
        .initialize(&activation, &endpoint_ref, &connection)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.mcp_initialize_timeout");
    host.shutdown().await;
}

#[tokio::test]
async fn stale_runtime_start_identity_is_rejected_before_connecting() {
    let (activation, endpoint_ref) = activation().await;
    let observation = activation.observation();
    let connection = RuntimeMcpHttpConnection::new(
        endpoint_ref.clone(),
        observation.unit_id.clone(),
        observation.generation,
        observation.started_at_ms.unwrap() + 1,
        Url::parse("http://127.0.0.1:9/mcp").unwrap(),
        RuntimeMcpBearerToken::parse(TOKEN).unwrap(),
    )
    .unwrap();
    let initializer = RuntimeMcpInitializer::new(25).unwrap();

    let error = initializer
        .initialize(&activation, &endpoint_ref, &connection)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.runtime.mcp_connection_mismatch");
}

#[test]
fn mcp_connection_rejects_remote_plaintext_and_redacts_secrets() {
    let endpoint_ref = RuntimeEndpointRef::parse("gateway:workspace-01/library").unwrap();
    let token = RuntimeMcpBearerToken::parse(TOKEN).unwrap();
    assert!(!format!("{token:?}").contains(TOKEN));

    let error = RuntimeMcpHttpConnection::new(
        endpoint_ref.clone(),
        "unit-01",
        1,
        1,
        Url::parse("http://example.com/mcp").unwrap(),
        token.clone(),
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.mcp_connection_invalid");

    let connection = RuntimeMcpHttpConnection::new(
        endpoint_ref,
        "unit-01",
        1,
        1,
        Url::parse("https://gateway.internal/mcp").unwrap(),
        token,
    )
    .unwrap();
    let debug = format!("{connection:?}");
    assert!(!debug.contains(TOKEN));
    assert!(!debug.contains("gateway.internal"));
}

#[test]
fn mcp_initializer_types_are_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeMcpBearerToken>();
    assert_send_sync::<RuntimeMcpHttpConnection>();
    assert_send_sync::<RuntimeMcpInitializer>();
}

async fn activation() -> (RuntimeServiceActivation, RuntimeEndpointRef) {
    let descriptor = mcp_descriptor();
    let plan = plan_mcp_service_release(
        context(PluginSurfaceKind::Mcp, "library"),
        &mcp_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(capabilities, true)));
    let activation = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap();
    (
        activation,
        RuntimeEndpointRef::parse("gateway:workspace-01/library").unwrap(),
    )
}

fn connection(
    activation: &RuntimeServiceActivation,
    endpoint_ref: &RuntimeEndpointRef,
    endpoint: Url,
) -> RuntimeMcpHttpConnection {
    let observation = activation.observation();
    RuntimeMcpHttpConnection::new(
        endpoint_ref.clone(),
        observation.unit_id.clone(),
        observation.generation,
        observation.started_at_ms.unwrap(),
        endpoint,
        RuntimeMcpBearerToken::parse(TOKEN).unwrap(),
    )
    .unwrap()
}

#[derive(Clone)]
struct NegotiatingServer {
    protocol_version: ProtocolVersion,
}

impl ServerHandler for NegotiatingServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: self.protocol_version.clone(),
            capabilities: ServerCapabilities::default(),
            server_info: Implementation {
                name: "a3s-use-test-mcp".to_string(),
                title: None,
                version: "1.0.0".to_string(),
                icons: None,
                website_url: None,
            },
            instructions: None,
        }
    }
}

struct TestMcpHost {
    endpoint: Url,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl TestMcpHost {
    async fn spawn(server_protocol: &str) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = CancellationToken::new();
        let service: StreamableHttpService<NegotiatingServer, LocalSessionManager> =
            StreamableHttpService::new(
                {
                    let server = NegotiatingServer {
                        protocol_version: protocol_version(server_protocol),
                    };
                    move || Ok(server.clone())
                },
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig {
                    stateful_mode: true,
                    sse_keep_alive: Some(Duration::from_secs(15)),
                },
            );
        let router = Router::new()
            .nest_service("/mcp", service)
            .layer(middleware::from_fn(authorize));
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move { shutdown.cancelled().await })
                .await
                .unwrap();
        });
        Self {
            endpoint: Url::parse(&format!("http://127.0.0.1:{}/mcp", address.port())).unwrap(),
            cancellation,
            task: Some(task),
        }
    }

    async fn shutdown(mut self) {
        self.cancellation.cancel();
        let task = self.task.take().unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for TestMcpHost {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

struct HangingHttpHost {
    endpoint: Url,
    cancellation: CancellationToken,
    task: Option<JoinHandle<()>>,
}

impl HangingHttpHost {
    async fn spawn() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let cancellation = CancellationToken::new();
        let shutdown = cancellation.clone();
        let task = tokio::spawn(async move {
            tokio::select! {
                _ = shutdown.cancelled() => {}
                accepted = listener.accept() => {
                    let (_stream, _) = accepted.unwrap();
                    shutdown.cancelled().await;
                }
            }
        });
        Self {
            endpoint: Url::parse(&format!("http://127.0.0.1:{}/mcp", address.port())).unwrap(),
            cancellation,
            task: Some(task),
        }
    }

    async fn shutdown(mut self) {
        self.cancellation.cancel();
        let task = self.task.take().unwrap();
        tokio::time::timeout(Duration::from_secs(2), task)
            .await
            .unwrap()
            .unwrap();
    }
}

impl Drop for HangingHttpHost {
    fn drop(&mut self) {
        self.cancellation.cancel();
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn authorize(request: Request, next: Next) -> Response {
    let expected = format!("Bearer {TOKEN}");
    if request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        != Some(expected.as_str())
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    next.run(request).await
}

fn protocol_version(value: &str) -> ProtocolVersion {
    serde_json::from_value(serde_json::Value::String(value.to_string())).unwrap()
}
