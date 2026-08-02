use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{
    CallToolResult, Implementation, ProtocolVersion, ServerCapabilities, ServerInfo,
};
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{tool, tool_handler, tool_router, ServerHandler};
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use a3s_use_mcp_release_fixture::MCP_PROTOCOL_VERSION;

const BIND_ENV: &str = "A3S_MCP_FIXTURE_BIND";
const IDENTITY_ENV: &str = "A3S_MCP_FIXTURE_RELEASE_IDENTITY";
const READY_FILE_ENV: &str = "A3S_MCP_FIXTURE_READY_FILE";

#[derive(Debug, Clone)]
struct FixtureState {
    release_identity: Arc<str>,
    shutdown: CancellationToken,
}

#[derive(Debug, Clone)]
struct FixtureMcpServer {
    state: FixtureState,
    tool_router: ToolRouter<Self>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct HealthResponse {
    status: &'static str,
    protocol_version: &'static str,
    release_identity: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReadyReceipt<'a> {
    schema: &'static str,
    endpoint: &'a str,
    health_endpoint: &'a str,
    pid: u32,
    protocol_version: &'static str,
    release_identity: &'a str,
}

impl FixtureMcpServer {
    fn new(state: FixtureState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl FixtureMcpServer {
    #[tool(
        name = "fixture_ping",
        description = "Return immutable MCP release identity and protocol evidence"
    )]
    async fn fixture_ping(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        Ok(CallToolResult::structured(serde_json::json!({
            "ok": true,
            "protocolVersion": MCP_PROTOCOL_VERSION,
            "releaseIdentity": self.state.release_identity.as_ref(),
        })))
    }

    #[tool(
        name = "fixture_shutdown",
        description = "Gracefully terminate the headless conformance fixture"
    )]
    async fn fixture_shutdown(&self) -> Result<CallToolResult, rmcp::ErrorData> {
        let shutdown = self.state.shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            shutdown.cancel();
        });
        Ok(CallToolResult::structured(serde_json::json!({
            "stopping": true,
            "releaseIdentity": self.state.release_identity.as_ref(),
        })))
    }
}

#[tool_handler]
impl ServerHandler for FixtureMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            server_info: Implementation {
                name: "a3s-use-mcp-release-fixture".to_string(),
                title: Some("A3S Use MCP Release Fixture".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_string()),
            },
            instructions: Some(
                "Conformance-only server. Call fixture_ping, then fixture_shutdown.".to_string(),
            ),
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let bind = std::env::var(BIND_ENV).unwrap_or_else(|_| "0.0.0.0:8080".to_string());
    let bind = bind
        .parse::<SocketAddr>()
        .with_context(|| format!("{BIND_ENV} must be a numeric socket address"))?;
    let release_identity =
        std::env::var(IDENTITY_ENV).with_context(|| format!("{IDENTITY_ENV} is required"))?;
    validate_release_identity(&release_identity)?;
    let ready_file = std::env::var_os(READY_FILE_ENV).map(PathBuf::from);
    if ready_file
        .as_deref()
        .is_some_and(|path| !path.is_absolute())
    {
        bail!("{READY_FILE_ENV} must be an absolute path");
    }

    let listener = tokio::net::TcpListener::bind(bind)
        .await
        .with_context(|| format!("failed to bind MCP fixture at {bind}"))?;
    let address = listener
        .local_addr()
        .context("failed to read MCP fixture listener address")?;
    let host = match address.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => "127.0.0.1".to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => "[::1]".to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
        IpAddr::V4(ip) => ip.to_string(),
    };
    let base = format!("http://{host}:{}", address.port());
    let endpoint = format!("{base}/mcp");
    let health_endpoint = format!("{base}/healthz");
    let shutdown = CancellationToken::new();
    let state = FixtureState {
        release_identity: release_identity.clone().into(),
        shutdown: shutdown.clone(),
    };
    let mcp: StreamableHttpService<FixtureMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            {
                let state = state.clone();
                move || Ok(FixtureMcpServer::new(state.clone()))
            },
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: None,
            },
        );
    let router = Router::new()
        .route("/healthz", get(health))
        .nest_service("/mcp", mcp)
        .with_state(state.clone());

    if let Some(path) = ready_file.as_deref() {
        write_ready_file(
            path,
            &ReadyReceipt {
                schema: "a3s.use.mcp-release-fixture-ready.v1",
                endpoint: &endpoint,
                health_endpoint: &health_endpoint,
                pid: std::process::id(),
                protocol_version: MCP_PROTOCOL_VERSION,
                release_identity: &release_identity,
            },
        )
        .await?;
    }

    let signal_shutdown = shutdown.clone();
    tokio::spawn(async move {
        wait_for_shutdown_signal().await;
        signal_shutdown.cancel();
    });
    let result = axum::serve(listener, router)
        .with_graceful_shutdown(shutdown.cancelled_owned())
        .await
        .context("MCP fixture server failed");
    if let Some(path) = ready_file.as_deref() {
        let _ = tokio::fs::remove_file(path).await;
    }
    result
}

async fn health(State(state): State<FixtureState>) -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "ok",
        protocol_version: MCP_PROTOCOL_VERSION,
        release_identity: state.release_identity.to_string(),
    })
}

fn validate_release_identity(identity: &str) -> anyhow::Result<()> {
    let valid = identity.len() == 71
        && identity.starts_with("sha256:")
        && identity[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !valid {
        bail!("{IDENTITY_ENV} must be a lowercase SHA-256 digest");
    }
    Ok(())
}

async fn write_ready_file(path: &Path, receipt: &ReadyReceipt<'_>) -> anyhow::Result<()> {
    let parent = path
        .parent()
        .context("MCP fixture readiness path has no parent")?;
    tokio::fs::create_dir_all(parent).await.with_context(|| {
        format!(
            "failed to create readiness directory '{}'",
            parent.display()
        )
    })?;
    let temporary = parent.join(format!(".mcp-release-ready-{}.tmp", std::process::id()));
    let bytes = serde_json::to_vec(receipt).context("failed to encode readiness receipt")?;
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .await
        .with_context(|| format!("failed to create readiness file '{}'", temporary.display()))?;
    file.write_all(&bytes)
        .await
        .context("failed to write readiness receipt")?;
    file.sync_all()
        .await
        .context("failed to sync readiness receipt")?;
    drop(file);
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error)
            .with_context(|| format!("failed to activate readiness receipt '{}'", path.display()));
    }
    Ok(())
}

#[cfg(unix)]
async fn wait_for_shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};

    let Ok(mut terminate) = signal(SignalKind::terminate()) else {
        let _ = tokio::signal::ctrl_c().await;
        return;
    };
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
}

#[cfg(not(unix))]
async fn wait_for_shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
