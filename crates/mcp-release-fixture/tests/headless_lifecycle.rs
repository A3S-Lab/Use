use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use a3s_use_mcp_release_fixture::{render_mcp_release, MCP_PROTOCOL_VERSION};
use rmcp::model::{
    CallToolRequestParam, ClientCapabilities, ClientInfo, Implementation, ProtocolVersion,
};
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::StreamableHttpClientTransport;
use rmcp::{ClientHandler, ServiceExt};
use serde::Deserialize;

const START_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(5);
const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ReadyReceipt {
    schema: String,
    endpoint: String,
    health_endpoint: String,
    pid: u32,
    protocol_version: String,
    release_identity: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: String,
    protocol_version: String,
    release_identity: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PingResponse {
    ok: bool,
    protocol_version: String,
    release_identity: String,
}

#[derive(Debug, Clone, Default)]
struct ConformanceClient;

impl ClientHandler for ConformanceClient {
    fn get_info(&self) -> ClientInfo {
        ClientInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "a3s-use-mcp-release-conformance".to_string(),
                title: Some("A3S Use MCP Release Conformance".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_string()),
            },
        }
    }
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn wait_for_exit(&mut self, timeout: Duration) -> std::process::ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let status = self.child.wait().unwrap();
                let diagnostics = read_stderr(&mut self.child);
                panic!("fixture did not exit within {timeout:?}: {status}; {diagnostics}");
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixture_passes_headless_health_request_shutdown_and_restart() {
    let rendered = render_mcp_release(
        "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        4_096,
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let readiness = temporary.path().join("ready.json");

    let (mut first, first_ready) = start_fixture(&readiness, &rendered.descriptor_digest).await;
    probe(
        &first_ready.endpoint,
        &first_ready.health_endpoint,
        &rendered.descriptor_digest,
        true,
    )
    .await;
    assert!(first.wait_for_exit(SHUTDOWN_TIMEOUT).success());
    wait_until_removed(&readiness).await;

    let (mut second, second_ready) = start_fixture(&readiness, &rendered.descriptor_digest).await;
    assert_ne!(first_ready.pid, second_ready.pid);
    assert_eq!(first_ready.release_identity, second_ready.release_identity);
    probe(
        &second_ready.endpoint,
        &second_ready.health_endpoint,
        &rendered.descriptor_digest,
        true,
    )
    .await;
    assert!(second.wait_for_exit(SHUTDOWN_TIMEOUT).success());
    wait_until_removed(&readiness).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn external_digest_pinned_container_conforms() {
    let Ok(endpoint) = std::env::var("A3S_MCP_CONFORMANCE_ENDPOINT") else {
        return;
    };
    let identity = std::env::var("A3S_MCP_CONFORMANCE_RELEASE_IDENTITY")
        .expect("container conformance requires the rendered descriptor identity");
    let health_endpoint = endpoint
        .strip_suffix("/mcp")
        .map(|base| format!("{base}/healthz"))
        .expect("container MCP endpoint must end in /mcp");
    probe(&endpoint, &health_endpoint, &identity, false).await;
}

async fn start_fixture(path: &Path, identity: &str) -> (ChildGuard, ReadyReceipt) {
    let child = Command::new(env!("CARGO_BIN_EXE_a3s-use-mcp-release-fixture"))
        .env("A3S_MCP_FIXTURE_BIND", "127.0.0.1:0")
        .env("A3S_MCP_FIXTURE_READY_FILE", path)
        .env("A3S_MCP_FIXTURE_RELEASE_IDENTITY", identity)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut guard = ChildGuard { child };
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(bytes) = tokio::fs::read(path).await {
            let receipt: ReadyReceipt = serde_json::from_slice(&bytes).unwrap();
            assert_eq!(receipt.schema, "a3s.use.mcp-release-fixture-ready.v1");
            assert_eq!(receipt.protocol_version, MCP_PROTOCOL_VERSION);
            assert_eq!(receipt.release_identity, identity);
            assert_eq!(receipt.pid, guard.child.id());
            return (guard, receipt);
        }
        if let Some(status) = guard.child.try_wait().unwrap() {
            let diagnostics = read_stderr(&mut guard.child);
            panic!("fixture exited before readiness with {status}: {diagnostics}");
        }
        assert!(
            Instant::now() < deadline,
            "fixture did not publish readiness"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

async fn probe(endpoint: &str, health_endpoint: &str, identity: &str, request_shutdown: bool) {
    let health = wait_for_health(health_endpoint).await;
    assert_eq!(health.status, "ok");
    assert_eq!(health.protocol_version, MCP_PROTOCOL_VERSION);
    assert_eq!(health.release_identity, identity);

    let transport = StreamableHttpClientTransport::from_config(
        StreamableHttpClientTransportConfig::with_uri(endpoint.to_string()),
    );
    let client = tokio::time::timeout(REQUEST_TIMEOUT, ConformanceClient.serve(transport))
        .await
        .expect("MCP initialize timed out")
        .expect("MCP initialize failed");
    let peer = client
        .peer_info()
        .expect("MCP initialize returned no server info");
    assert_eq!(peer.protocol_version.to_string(), MCP_PROTOCOL_VERSION);
    assert_eq!(peer.server_info.name, "a3s-use-mcp-release-fixture");

    let tools = tokio::time::timeout(REQUEST_TIMEOUT, client.list_all_tools())
        .await
        .expect("MCP tools/list timed out")
        .expect("MCP tools/list failed");
    let names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        names,
        ["fixture_ping", "fixture_shutdown"].into_iter().collect()
    );

    let ping = tokio::time::timeout(
        REQUEST_TIMEOUT,
        client.call_tool(CallToolRequestParam {
            name: "fixture_ping".into(),
            arguments: Some(serde_json::Map::new()),
        }),
    )
    .await
    .expect("MCP fixture_ping timed out")
    .expect("MCP fixture_ping failed");
    assert_eq!(ping.is_error, Some(false));
    let ping: PingResponse = serde_json::from_value(
        ping.structured_content
            .expect("MCP fixture_ping returned no structured content"),
    )
    .unwrap();
    assert!(ping.ok);
    assert_eq!(ping.protocol_version, MCP_PROTOCOL_VERSION);
    assert_eq!(ping.release_identity, identity);

    if request_shutdown {
        let shutdown = tokio::time::timeout(
            REQUEST_TIMEOUT,
            client.call_tool(CallToolRequestParam {
                name: "fixture_shutdown".into(),
                arguments: Some(serde_json::Map::new()),
            }),
        )
        .await
        .expect("MCP fixture_shutdown timed out")
        .expect("MCP fixture_shutdown failed");
        assert_eq!(shutdown.is_error, Some(false));
    }
    let _ = client.cancel().await;
}

async fn wait_for_health(endpoint: &str) -> HealthResponse {
    let client = reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .unwrap();
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Ok(response) = client.get(endpoint).send().await {
            if response.status().is_success() {
                return response.json::<HealthResponse>().await.unwrap();
            }
        }
        assert!(
            Instant::now() < deadline,
            "MCP health gate did not converge"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn wait_until_removed(path: &Path) {
    let deadline = Instant::now() + SHUTDOWN_TIMEOUT;
    while path.exists() {
        assert!(
            Instant::now() < deadline,
            "fixture readiness receipt was not cleaned up"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn read_stderr(child: &mut Child) -> String {
    let mut diagnostics = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut diagnostics);
    }
    diagnostics
}
