//! Private ownership receipt and startup lock for the Browser MCP deployment.

use std::fs::OpenOptions as StdOpenOptions;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use url::Url;

pub(super) const RECEIPT_SCHEMA_VERSION: u32 = 1;
const RECEIPT_FILE: &str = "browser-mcp.json";
const START_LOCK_FILE: &str = "browser-mcp.lock";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub(super) struct BrowserServiceReceipt {
    pub(super) schema_version: u32,
    pub(super) protocol: String,
    pub(super) endpoint: String,
    pub(super) token: String,
    pub(super) pid: u32,
    pub(super) started_at_ms: u64,
}

pub(super) struct ServicePaths {
    pub(super) root: PathBuf,
    pub(super) receipt: PathBuf,
    pub(super) lock: PathBuf,
}

impl ServicePaths {
    pub(super) fn discover() -> UseResult<Self> {
        if let Some(path) = std::env::var_os("A3S_USE_RUNTIME_DIR") {
            let path = PathBuf::from(path);
            validate_runtime_root(&path)?;
            return Ok(Self::at(path));
        }
        if let Some(path) = std::env::var_os("XDG_RUNTIME_DIR") {
            let path = PathBuf::from(path).join("a3s-use");
            validate_runtime_root(&path)?;
            return Ok(Self::at(path));
        }
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .ok_or_else(|| {
                UseError::new(
                    "use.mcp.runtime_dir_missing",
                    "Cannot resolve a private Browser MCP runtime directory.",
                )
                .with_suggestion("Set A3S_USE_RUNTIME_DIR to a private local directory.")
            })?;
        let path = home.join(".a3s").join("use").join("run");
        validate_runtime_root(&path)?;
        Ok(Self::at(path))
    }

    pub(super) fn at(root: PathBuf) -> Self {
        Self {
            receipt: root.join(RECEIPT_FILE),
            lock: root.join(START_LOCK_FILE),
            root,
        }
    }
}

pub(super) async fn acquire_start_lock(path: PathBuf) -> UseResult<std::fs::File> {
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|error| service_path_io("open Browser MCP startup lock", &path, error))?;
        file.lock_exclusive()
            .map_err(|error| service_path_io("lock Browser MCP startup", &path, error))?;
        Ok(file)
    })
    .await
    .map_err(|error| {
        UseError::new(
            "use.mcp.lock_failed",
            format!("Browser MCP startup lock task failed: {error}"),
        )
    })?
}

pub(super) async fn prepare_runtime_dir(path: &Path) -> UseResult<()> {
    validate_runtime_root(path)?;
    tokio::fs::create_dir_all(path)
        .await
        .map_err(|error| service_path_io("create Browser MCP runtime directory", path, error))?;
    let metadata = tokio::fs::symlink_metadata(path)
        .await
        .map_err(|error| service_path_io("inspect Browser MCP runtime directory", path, error))?;
    if !metadata.file_type().is_dir() {
        return Err(UseError::new(
            "use.mcp.runtime_dir_invalid",
            format!(
                "Browser MCP runtime path '{}' is not a real directory.",
                path.display()
            ),
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
            .await
            .map_err(|error| {
                service_path_io("secure Browser MCP runtime directory", path, error)
            })?;
    }
    Ok(())
}

pub(super) fn validate_runtime_root(path: &Path) -> UseResult<()> {
    if path.is_absolute() {
        Ok(())
    } else {
        Err(UseError::new(
            "use.mcp.runtime_dir_invalid",
            format!(
                "Browser MCP runtime directory '{}' must be absolute.",
                path.display()
            ),
        ))
    }
}

pub(super) async fn write_receipt(path: &Path, receipt: &BrowserServiceReceipt) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        UseError::new(
            "use.mcp.receipt_invalid",
            "Browser MCP receipt path has no parent directory.",
        )
    })?;
    validate_receipt(receipt)?;
    let token_prefix = receipt.token.get(..16).ok_or_else(|| {
        UseError::new(
            "use.mcp.receipt_invalid",
            "Browser MCP receipt token has no safe filename prefix.",
        )
    })?;
    let bytes = serde_json::to_vec_pretty(receipt).map_err(|error| {
        UseError::new(
            "use.mcp.receipt_invalid",
            format!("Failed to encode Browser MCP receipt: {error}"),
        )
    })?;
    let temporary = parent.join(format!(".browser-mcp-{}-{}.tmp", receipt.pid, token_prefix));
    let mut options = tokio::fs::OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        options.mode(0o600);
    }
    let mut file = options.open(&temporary).await.map_err(|error| {
        service_path_io("create temporary Browser MCP receipt", &temporary, error)
    })?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(service_path_io(
            "write temporary Browser MCP receipt",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(service_path_io(
            "sync temporary Browser MCP receipt",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = tokio::fs::hard_link(&temporary, path).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(service_path_io("activate Browser MCP receipt", path, error));
    }
    tokio::fs::remove_file(&temporary).await.map_err(|error| {
        service_path_io("remove temporary Browser MCP receipt", &temporary, error)
    })?;
    Ok(())
}

pub(super) async fn read_optional_receipt(path: &Path) -> UseResult<Option<BrowserServiceReceipt>> {
    let metadata = match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(service_path_io("inspect Browser MCP receipt", path, error)),
    };
    if !metadata.file_type().is_file() || metadata.len() > 16 * 1024 {
        return Err(UseError::new(
            "use.mcp.receipt_invalid",
            format!(
                "Browser MCP receipt '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    let bytes = tokio::fs::read(path)
        .await
        .map_err(|error| service_path_io("read Browser MCP receipt", path, error))?;
    let receipt = serde_json::from_slice::<BrowserServiceReceipt>(&bytes).map_err(|error| {
        UseError::new(
            "use.mcp.receipt_invalid",
            format!(
                "Browser MCP receipt '{}' is invalid: {error}",
                path.display()
            ),
        )
    })?;
    validate_receipt(&receipt)?;
    Ok(Some(receipt))
}

pub(super) fn validate_receipt(receipt: &BrowserServiceReceipt) -> UseResult<()> {
    let endpoint = Url::parse(&receipt.endpoint).map_err(|error| {
        UseError::new(
            "use.mcp.receipt_invalid",
            format!("Browser MCP receipt endpoint is invalid: {error}"),
        )
    })?;
    let valid_endpoint = endpoint.scheme() == "http"
        && endpoint.host_str() == Some("127.0.0.1")
        && endpoint.port().is_some()
        && endpoint.path() == "/mcp"
        && endpoint.query().is_none()
        && endpoint.fragment().is_none()
        && endpoint.username().is_empty()
        && endpoint.password().is_none();
    let valid_token = receipt.token.len() == 64
        && receipt
            .token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if receipt.schema_version != RECEIPT_SCHEMA_VERSION
        || receipt.protocol != "mcp-streamable-http"
        || receipt.pid == 0
        || receipt.started_at_ms == 0
        || !valid_endpoint
        || !valid_token
    {
        return Err(UseError::new(
            "use.mcp.receipt_invalid",
            "Browser MCP receipt failed its ownership or loopback validation.",
        ));
    }
    Ok(())
}

pub(super) async fn remove_receipt_if_current(
    path: &Path,
    expected: &BrowserServiceReceipt,
) -> UseResult<()> {
    match read_optional_receipt(path).await? {
        Some(current) if &current == expected => tokio::fs::remove_file(path)
            .await
            .map_err(|error| service_path_io("remove Browser MCP receipt", path, error)),
        Some(_) => Err(UseError::new(
            "use.mcp.service_replaced",
            "Browser MCP receipt changed before cleanup and was preserved.",
        )),
        None => Ok(()),
    }
}

pub(super) fn random_token() -> UseResult<String> {
    let mut bytes = [0_u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| {
        UseError::new(
            "use.mcp.token_failed",
            format!("Failed to generate Browser MCP bearer token: {error}"),
        )
    })?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn service_path_io(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.mcp.service_io_failed",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_receipt(endpoint: &str) -> BrowserServiceReceipt {
        BrowserServiceReceipt {
            schema_version: RECEIPT_SCHEMA_VERSION,
            protocol: "mcp-streamable-http".to_string(),
            endpoint: endpoint.to_string(),
            token: "ab".repeat(32),
            pid: 42,
            started_at_ms: 1,
        }
    }

    #[test]
    fn receipt_accepts_only_authenticated_loopback_mcp_endpoints() {
        assert!(validate_receipt(&fixture_receipt("http://127.0.0.1:39123/mcp")).is_ok());
        assert_eq!(
            validate_receipt(&fixture_receipt("https://example.com/mcp"))
                .unwrap_err()
                .code,
            "use.mcp.receipt_invalid"
        );
        let mut receipt = fixture_receipt("http://127.0.0.1:39123/mcp");
        receipt.token = "short".to_string();
        assert!(validate_receipt(&receipt).is_err());
    }

    #[tokio::test]
    async fn receipt_write_rejects_an_invalid_token_without_panicking() {
        let mut receipt = fixture_receipt("http://127.0.0.1:39123/mcp");
        receipt.token = "short".to_string();

        let error = write_receipt(Path::new("unused/browser-mcp.json"), &receipt)
            .await
            .unwrap_err();

        assert_eq!(error.code, "use.mcp.receipt_invalid");
    }
}
