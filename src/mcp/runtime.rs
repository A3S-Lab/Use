//! Authenticated loopback deployment for the standard Browser MCP server.

mod client;
mod receipt;

use std::path::PathBuf;
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use a3s_use_browser::{BrowserPool, BrowserPoolConfig, BrowserSessions};
use a3s_use_core::{UseError, UseResult};
use axum::extract::{Request, State};
use axum::http::header::{AUTHORIZATION, ORIGIN};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio_util::sync::CancellationToken;

use super::browser::BrowserMcpServer;
use client::{call_with_receipt, probe};
use receipt::{
    acquire_start_lock, prepare_runtime_dir, random_token, read_optional_receipt,
    remove_receipt_if_current, unix_time_ms, validate_runtime_root, write_receipt,
    BrowserServiceReceipt, ServicePaths, RECEIPT_SCHEMA_VERSION,
};

const START_TIMEOUT: Duration = Duration::from_secs(10);
const STOP_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const DEFAULT_MAX_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BrowserServiceStatus {
    pub running: bool,
    pub stopped: bool,
    pub protocol: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pid: Option<u32>,
    pub receipt_path: PathBuf,
}

#[derive(Clone)]
struct HttpGuard {
    authorization: Arc<str>,
    origin: Arc<str>,
    last_activity: Arc<Mutex<Instant>>,
}

pub(crate) async fn ensure_browser_service() -> UseResult<BrowserServiceStatus> {
    let paths = ServicePaths::discover()?;
    let receipt = ensure_receipt(&paths).await?;
    Ok(status(&paths, Some(&receipt), true, false))
}

pub(crate) async fn browser_service_status() -> UseResult<BrowserServiceStatus> {
    let paths = ServicePaths::discover()?;
    let receipt = read_optional_receipt(&paths.receipt).await?;
    let running = match receipt.as_ref() {
        Some(receipt) => probe(receipt).await,
        None => false,
    };
    Ok(status(&paths, receipt.as_ref(), running, false))
}

pub(crate) async fn call_browser_tool<T>(
    name: &'static str,
    arguments: serde_json::Value,
) -> UseResult<T>
where
    T: DeserializeOwned,
{
    let paths = ServicePaths::discover()?;
    let receipt = ensure_receipt(&paths).await?;
    call_with_receipt(&receipt, name, arguments).await
}

pub(crate) async fn stop_browser_service() -> UseResult<BrowserServiceStatus> {
    let paths = ServicePaths::discover()?;
    prepare_runtime_dir(&paths.root).await?;
    let _lock = acquire_start_lock(paths.lock.clone()).await?;
    let Some(receipt) = read_optional_receipt(&paths.receipt).await? else {
        return Ok(status(&paths, None, false, false));
    };

    if !probe(&receipt).await {
        remove_receipt_if_current(&paths.receipt, &receipt).await?;
        return Ok(status(&paths, None, false, false));
    }

    let _: serde_json::Value =
        call_with_receipt(&receipt, "browser_service_stop", serde_json::json!({})).await?;
    let deadline = Instant::now() + STOP_TIMEOUT;
    loop {
        match read_optional_receipt(&paths.receipt).await? {
            None => return Ok(status(&paths, None, false, true)),
            Some(current) if current != receipt => {
                return Err(UseError::new(
                    "use.mcp.service_replaced",
                    "The Browser MCP receipt changed while the previous service was stopping.",
                ))
            }
            Some(_) if Instant::now() >= deadline => {
                return Err(UseError::new(
                    "use.mcp.stop_timeout",
                    "The Browser MCP service did not stop within five seconds.",
                ))
            }
            Some(_) => tokio::time::sleep(Duration::from_millis(50)).await,
        }
    }
}

pub(crate) async fn serve_browser_http(runtime_dir: PathBuf) -> UseResult<()> {
    validate_runtime_root(&runtime_dir)?;
    let paths = ServicePaths::at(runtime_dir);
    prepare_runtime_dir(&paths.root).await?;
    if read_optional_receipt(&paths.receipt).await?.is_some() {
        return Err(UseError::new(
            "use.mcp.already_running",
            format!(
                "A Browser MCP receipt already exists at '{}'.",
                paths.receipt.display()
            ),
        )
        .with_suggestion("Use 'a3s-use mcp stop' before starting another service."));
    }

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| service_io("bind the Browser MCP loopback listener", error))?;
    let address = listener
        .local_addr()
        .map_err(|error| service_io("read the Browser MCP listener address", error))?;
    let endpoint = format!("http://127.0.0.1:{}/mcp", address.port());
    let token = random_token()?;
    let receipt = BrowserServiceReceipt {
        schema_version: RECEIPT_SCHEMA_VERSION,
        protocol: "mcp-streamable-http".to_string(),
        endpoint,
        token: token.clone(),
        pid: std::process::id(),
        started_at_ms: unix_time_ms(),
    };
    write_receipt(&paths.receipt, &receipt).await?;

    let shutdown = CancellationToken::new();
    let pool = Arc::new(BrowserPool::new(BrowserPoolConfig::default()));
    let sessions = Arc::new(BrowserSessions::new(Arc::clone(&pool)));
    let server = BrowserMcpServer::persistent(pool, Arc::clone(&sessions), shutdown.clone());
    let service: StreamableHttpService<BrowserMcpServer, LocalSessionManager> =
        StreamableHttpService::new(
            move || Ok(server.clone()),
            Arc::new(LocalSessionManager::default()),
            StreamableHttpServerConfig {
                stateful_mode: true,
                sse_keep_alive: Some(Duration::from_secs(15)),
            },
        );
    let last_activity = Arc::new(Mutex::new(Instant::now()));
    let guard = HttpGuard {
        authorization: format!("Bearer {token}").into(),
        origin: format!("http://127.0.0.1:{}", address.port()).into(),
        last_activity: Arc::clone(&last_activity),
    };
    let router = Router::new()
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(guard, authorize_request));
    let lifecycle = tokio::spawn(monitor_lifecycle(
        shutdown.clone(),
        last_activity,
        DEFAULT_IDLE_TIMEOUT,
        DEFAULT_MAX_LIFETIME,
    ));
    let result = axum::serve(listener, router)
        .with_graceful_shutdown({
            let shutdown = shutdown.clone();
            async move { shutdown.cancelled_owned().await }
        })
        .await
        .map_err(|error| service_io("serve Browser MCP over loopback HTTP", error));

    shutdown.cancel();
    let _ = lifecycle.await;
    sessions.shutdown().await;
    let receipt_cleanup = remove_receipt_if_current(&paths.receipt, &receipt).await;
    match (result, receipt_cleanup) {
        (Err(error), _) => Err(error),
        (Ok(()), Err(error)) => Err(error),
        (Ok(()), Ok(())) => Ok(()),
    }
}

async fn ensure_receipt(paths: &ServicePaths) -> UseResult<BrowserServiceReceipt> {
    prepare_runtime_dir(&paths.root).await?;
    let _lock = acquire_start_lock(paths.lock.clone()).await?;
    if let Some(receipt) = read_optional_receipt(&paths.receipt).await? {
        if probe(&receipt).await {
            return Ok(receipt);
        }
        remove_receipt_if_current(&paths.receipt, &receipt).await?;
    }

    let executable = std::env::current_exe().map_err(|error| {
        UseError::new(
            "use.mcp.start_failed",
            format!("Failed to locate the a3s-use executable: {error}"),
        )
    })?;
    let mut child = tokio::process::Command::new(executable)
        .args([
            "mcp",
            "serve",
            "browser",
            "--streamable-http",
            "--runtime-dir",
        ])
        .arg(&paths.root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(false)
        .spawn()
        .map_err(|error| {
            UseError::new(
                "use.mcp.start_failed",
                format!("Failed to start the Browser MCP service: {error}"),
            )
        })?;
    let expected_pid = child.id();
    let deadline = Instant::now() + START_TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().map_err(|error| {
            UseError::new(
                "use.mcp.start_failed",
                format!("Failed to inspect the Browser MCP service process: {error}"),
            )
        })? {
            return Err(UseError::new(
                "use.mcp.start_failed",
                format!("Browser MCP service exited during startup with {status}."),
            ));
        }
        if let Some(receipt) = read_optional_receipt(&paths.receipt).await? {
            if expected_pid.is_some_and(|pid| pid != receipt.pid) {
                let _ = child.kill().await;
                let _ = child.wait().await;
                return Err(UseError::new(
                    "use.mcp.service_replaced",
                    "Another Browser MCP process wrote the startup receipt.",
                ));
            }
            if probe(&receipt).await {
                return Ok(receipt);
            }
        }
        if Instant::now() >= deadline {
            let _ = child.kill().await;
            let _ = child.wait().await;
            if let Some(receipt) = read_optional_receipt(&paths.receipt).await? {
                remove_receipt_if_current(&paths.receipt, &receipt).await?;
            }
            return Err(UseError::new(
                "use.mcp.start_timeout",
                "Browser MCP service did not become ready within ten seconds.",
            ));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

async fn authorize_request(
    State(guard): State<HttpGuard>,
    request: Request,
    next: Next,
) -> Response {
    let authorized = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| constant_time_eq(value.as_bytes(), guard.authorization.as_bytes()));
    if !authorized {
        return (StatusCode::UNAUTHORIZED, "Unauthorized").into_response();
    }
    let trusted_origin = request
        .headers()
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_none_or(|value| constant_time_eq(value.as_bytes(), guard.origin.as_bytes()));
    if !trusted_origin {
        return (StatusCode::FORBIDDEN, "Untrusted Origin").into_response();
    }
    touch(&guard.last_activity);
    next.run(request).await
}

async fn monitor_lifecycle(
    shutdown: CancellationToken,
    last_activity: Arc<Mutex<Instant>>,
    idle_timeout: Duration,
    max_lifetime: Duration,
) {
    let started = Instant::now();
    loop {
        tokio::select! {
            _ = shutdown.cancelled() => return,
            _ = tokio::time::sleep(Duration::from_secs(1)) => {
                if started.elapsed() >= max_lifetime || elapsed_since(&last_activity) >= idle_timeout {
                    shutdown.cancel();
                    return;
                }
            }
        }
    }
}

fn touch(last_activity: &Mutex<Instant>) {
    match last_activity.lock() {
        Ok(mut value) => *value = Instant::now(),
        Err(poisoned) => *poisoned.into_inner() = Instant::now(),
    }
}

fn elapsed_since(last_activity: &Mutex<Instant>) -> Duration {
    match last_activity.lock() {
        Ok(value) => value.elapsed(),
        Err(poisoned) => poisoned.into_inner().elapsed(),
    }
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        let left = left.get(index).copied().unwrap_or_default();
        let right = right.get(index).copied().unwrap_or_default();
        difference |= usize::from(left ^ right);
    }
    difference == 0
}

fn status(
    paths: &ServicePaths,
    receipt: Option<&BrowserServiceReceipt>,
    running: bool,
    stopped: bool,
) -> BrowserServiceStatus {
    BrowserServiceStatus {
        running,
        stopped,
        protocol: "mcp-streamable-http",
        endpoint: receipt.map(|receipt| receipt.endpoint.clone()),
        pid: receipt.map(|receipt| receipt.pid),
        receipt_path: paths.receipt.clone(),
    }
}

fn service_io(action: &str, error: std::io::Error) -> UseError {
    UseError::new(
        "use.mcp.service_io_failed",
        format!("Failed to {action}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn bearer_comparison_checks_value_and_length() {
        assert!(constant_time_eq(b"Bearer secret", b"Bearer secret"));
        assert!(!constant_time_eq(b"Bearer secret", b"Bearer other"));
        assert!(!constant_time_eq(b"Bearer secret", b"Bearer secret-extra"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streamable_http_uses_standard_mcp_across_client_connections() {
        let temp = tempfile::tempdir().unwrap();
        let paths = ServicePaths::at(temp.path().to_path_buf());
        let runtime_dir = paths.root.clone();
        let server = tokio::spawn(async move { serve_browser_http(runtime_dir).await });
        let deadline = Instant::now() + Duration::from_secs(5);
        let receipt = loop {
            if let Some(receipt) = read_optional_receipt(&paths.receipt).await.unwrap() {
                break receipt;
            }
            assert!(Instant::now() < deadline, "service receipt was not created");
            tokio::time::sleep(Duration::from_millis(20)).await;
        };

        let unauthorized = raw_http_response(&receipt, "").await;
        assert!(unauthorized.starts_with("HTTP/1.1 401"));
        let untrusted_origin = raw_http_response(
            &receipt,
            &format!(
                "Authorization: Bearer {}\r\nOrigin: https://attacker.example\r\n",
                receipt.token
            ),
        )
        .await;
        assert!(untrusted_origin.starts_with("HTTP/1.1 403"));

        let first: Vec<a3s_use_browser::BrowserSessionInfo> =
            call_with_receipt(&receipt, "browser_list", serde_json::json!({}))
                .await
                .unwrap();
        let second: Vec<a3s_use_browser::BrowserSessionInfo> =
            call_with_receipt(&receipt, "browser_list", serde_json::json!({}))
                .await
                .unwrap();
        assert!(first.is_empty());
        assert_eq!(first, second);

        let stopped: serde_json::Value =
            call_with_receipt(&receipt, "browser_service_stop", serde_json::json!({}))
                .await
                .unwrap();
        assert_eq!(stopped["stopping"], true);
        server.await.unwrap().unwrap();
        assert!(!paths.receipt.exists());
    }

    async fn raw_http_response(receipt: &BrowserServiceReceipt, headers: &str) -> String {
        let endpoint = url::Url::parse(&receipt.endpoint).unwrap();
        let port = endpoint.port().unwrap();
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .unwrap();
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nContent-Length: 0\r\nConnection: close\r\n{headers}\r\n"
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = Vec::new();
        stream.read_to_end(&mut response).await.unwrap();
        String::from_utf8(response).unwrap()
    }
}
