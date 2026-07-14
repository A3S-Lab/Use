//! Automatic Lightpanda detection and installation.
//!
//! Only available when the `lightpanda` Cargo feature is enabled.
//! Lightpanda binaries are downloaded as plain executables from GitHub releases
//! (no zip extraction needed, unlike Chrome for Testing).
//!
//! Supported platforms:
//! - Linux x86_64
//! - Linux aarch64
//! - macOS x86_64
//! - macOS aarch64
//!
//! Downloaded binaries are stored under the A3S Use Browser data root.

use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use tracing::{debug, info};

use a3s_use_core::{UseError, UseResult};

use crate::pool::browser_error;

const MAX_LIGHTPANDA_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    assets: Vec<GitHubReleaseAsset>,
}

#[derive(Debug, Deserialize)]
struct GitHubReleaseAsset {
    name: String,
    browser_download_url: String,
    digest: Option<String>,
}

/// GitHub API endpoint for the latest Lightpanda release.
const LIGHTPANDA_RELEASES_API: &str =
    "https://api.github.com/repos/lightpanda-io/browser/releases/latest";

/// Returns the platform suffix used in Lightpanda release asset names.
///
/// Asset names follow the pattern `lightpanda-<platform>`, e.g.
/// `lightpanda-x86_64-linux` or `lightpanda-aarch64-macos`.
fn platform_id() -> UseResult<&'static str> {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return Ok("x86_64-linux");

    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return Ok("aarch64-linux");

    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return Ok("x86_64-macos");

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return Ok("aarch64-macos");

    #[cfg(not(any(
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "macos", target_arch = "aarch64"),
    )))]
    Err(browser_error(
        "Lightpanda does not provide binaries for this platform. \
         Supported platforms: Linux x86_64/aarch64, macOS x86_64/aarch64."
            .to_string(),
    ))
}

/// Base directory for cached Lightpanda downloads.
pub(crate) fn managed_cache_dir() -> UseResult<PathBuf> {
    Ok(crate::chrome::browser_data_root()?.join("lightpanda"))
}

/// Detect an existing Lightpanda installation.
///
/// Checks:
/// 1. `LIGHTPANDA` environment variable
/// 2. `lightpanda` command in PATH
///
/// Returns `Some(path)` if found, `None` otherwise.
pub fn detect_lightpanda() -> Option<PathBuf> {
    // 1. Check explicit environment overrides.
    for name in ["A3S_LIGHTPANDA_EXECUTABLE", "LIGHTPANDA"] {
        if let Some(path) = std::env::var_os(name).map(PathBuf::from) {
            if path.is_file() {
                debug!("Lightpanda found via {name}: {}", path.display());
                return Some(path);
            }
        }
    }

    // 2. Check PATH
    if let Ok(path) = which::which("lightpanda") {
        debug!("Lightpanda found in PATH: {}", path.display());
        return Some(path);
    }

    None
}

/// Look for a previously downloaded Lightpanda in the cache directory.
pub(crate) fn find_managed_lightpanda() -> UseResult<PathBuf> {
    let base = managed_cache_dir()?;
    if !base.exists() {
        return Err(browser_error("No cached Lightpanda found".to_string()));
    }

    // Collect version directories, newest first
    let mut versions: Vec<_> = std::fs::read_dir(&base)
        .map_err(|e| browser_error(format!("Failed to read cache dir: {}", e)))?
        .filter_map(|e| e.ok())
        .filter(|entry| {
            entry.path().is_dir() && !entry.file_name().to_string_lossy().starts_with('.')
        })
        .collect();

    versions.sort_by_key(|b| std::cmp::Reverse(b.file_name()));

    for version_dir in versions {
        let Some(version) = version_dir.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !crate::install::has_complete_receipt(&version_dir.path(), "lightpanda", &version) {
            continue;
        }
        let exe_path = version_dir.path().join("lightpanda");
        if exe_path.is_file() {
            return Ok(exe_path);
        }
    }

    Err(browser_error("No cached Lightpanda found".to_string()))
}

/// Ensure Lightpanda is available, downloading it if necessary.
///
/// 1. If Lightpanda is found via `LIGHTPANDA` env var or PATH, returns that path.
/// 2. If a cached download exists in `the A3S Use Browser data root/`, returns that path.
/// 3. Otherwise, downloads the latest release from GitHub and caches it.
pub async fn ensure_lightpanda() -> UseResult<PathBuf> {
    if let Some(path) = detect_lightpanda() {
        info!("Using system Lightpanda: {}", path.display());
        return Ok(path);
    }

    if let Ok(path) = find_managed_lightpanda() {
        info!("Using cached Lightpanda: {}", path.display());
        return Ok(path);
    }

    info!("Lightpanda not found, downloading latest release...");
    download_latest_lightpanda().await
}

pub(crate) fn resolve_lightpanda() -> UseResult<PathBuf> {
    detect_lightpanda()
        .or_else(|| find_managed_lightpanda().ok())
        .ok_or_else(|| {
            UseError::new(
                "use.browser.runtime_missing",
                "No compatible Lightpanda executable is installed.",
            )
            .with_suggestion(
                "Run 'a3s install use/browser' or select BrowserProvider::ManagedLightpanda.",
            )
        })
}

/// Download the latest Lightpanda binary from GitHub releases.
pub async fn download_latest_lightpanda() -> UseResult<PathBuf> {
    let platform = platform_id()?;
    let asset_name = format!("lightpanda-{}", platform);

    info!("Fetching Lightpanda release metadata");
    let releases_url =
        crate::install::trusted_https_url(LIGHTPANDA_RELEASES_API, &["api.github.com"])?;
    let client = reqwest::Client::builder()
        .user_agent("a3s-use")
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(10 * 60))
        .redirect(crate::install::approved_redirect_policy(&[
            "api.github.com",
            "github.com",
            "release-assets.githubusercontent.com",
        ]))
        .build()
        .map_err(|e| browser_error(format!("Failed to create HTTP client: {}", e)))?;

    let resp =
        client.get(releases_url).send().await.map_err(|e| {
            browser_error(format!("Failed to fetch Lightpanda release info: {}", e))
        })?;
    let resp = resp
        .error_for_status()
        .map_err(|e| browser_error(format!("Lightpanda release request failed: {e}")))?;

    let body: GitHubRelease = resp
        .json()
        .await
        .map_err(|e| browser_error(format!("Failed to parse Lightpanda release JSON: {}", e)))?;

    let tag = crate::install::validate_version_segment(&body.tag_name)?;
    let asset = body
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            browser_error(format!(
                "No Lightpanda binary for platform '{}' in release '{}'",
                platform, tag
            ))
        })?;
    let published_sha256 = asset
        .digest
        .as_deref()
        .ok_or_else(|| browser_error("Lightpanda release asset has no publisher digest."))
        .and_then(crate::install::parse_published_sha256)?;
    let download_url =
        crate::install::trusted_https_url(&asset.browser_download_url, &["github.com"])?;

    let cache_dir = managed_cache_dir()?;
    let _lock = crate::install::acquire_install_lock(&cache_dir).await?;
    crate::install::cleanup_stale_stages(&cache_dir).await?;
    let stage = crate::install::create_stage(&cache_dir, "lightpanda").await?;
    let staged_executable = stage.join("lightpanda");

    info!("Downloading Lightpanda {} ({})", tag, platform);
    let artifact = crate::install::download_to_file(
        &client,
        download_url.clone(),
        &staged_executable,
        MAX_LIGHTPANDA_BYTES,
    )
    .await?;
    if artifact.sha256 != published_sha256 {
        return Err(browser_error(format!(
            "Lightpanda download checksum mismatch: expected {published_sha256}, observed {}.",
            artifact.sha256
        )));
    }

    info!(
        "Downloaded {:.1} MB, installing...",
        artifact.bytes as f64 / 1_048_576.0
    );

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&staged_executable, std::fs::Permissions::from_mode(0o755))
            .await
            .map_err(|e| browser_error(format!("Failed to set Lightpanda permissions: {}", e)))?;
    }

    crate::install::write_receipt(
        &stage,
        &crate::install::ManagedInstallReceipt {
            schema_version: 1,
            provider: "lightpanda".to_string(),
            version: tag.to_string(),
            source_url: download_url.to_string(),
            artifact_sha256: artifact.sha256.clone(),
            artifact_bytes: artifact.bytes,
            executable_sha256: artifact.sha256,
            integrity_policy: "publisher-sha256+approved-https-source".to_string(),
        },
    )
    .await?;
    let version_dir = cache_dir.join(tag);
    crate::install::activate_directory(&stage, &version_dir).await?;
    let exe_path = version_dir.join("lightpanda");

    info!("Lightpanda {} installed at {}", tag, exe_path.display());
    info!("Lightpanda installed at: {}", exe_path.display());

    Ok(exe_path)
}
