//! Bounded and atomic Chrome for Testing installation.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use tracing::{info, warn};

use a3s_use_core::UseResult;

use crate::pool::browser_error;

const CHROME_VERSIONS_URL: &str =
    "https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json";
const MAX_CHROME_ARCHIVE_BYTES: u64 = 768 * 1024 * 1024;
const MAX_CHROME_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_CHROME_ARCHIVE_ENTRIES: usize = 50_000;

#[derive(Debug, Deserialize)]
struct ChromeVersions {
    channels: ChromeChannels,
}

#[derive(Debug, Deserialize)]
struct ChromeChannels {
    #[serde(rename = "Stable")]
    stable: ChromeChannel,
}

#[derive(Debug, Deserialize)]
struct ChromeChannel {
    version: String,
    downloads: ChromeDownloads,
}

#[derive(Debug, Deserialize)]
struct ChromeDownloads {
    chrome: Vec<ChromeDownload>,
}

#[derive(Debug, Deserialize)]
struct ChromeDownload {
    platform: String,
    url: String,
}

pub(crate) async fn download_latest_chrome() -> UseResult<PathBuf> {
    let platform = crate::chrome::platform_id()?;

    info!("Fetching Chrome for Testing version metadata");
    let metadata_url =
        crate::install::trusted_https_url(CHROME_VERSIONS_URL, &["googlechromelabs.github.io"])?;
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(15 * 60))
        .redirect(crate::install::approved_redirect_policy(&[
            "googlechromelabs.github.io",
            "storage.googleapis.com",
        ]))
        .build()
        .map_err(|error| browser_error(format!("Failed to create HTTP client: {error}")))?;
    let resp = client
        .get(metadata_url)
        .send()
        .await
        .map_err(|error| browser_error(format!("Failed to fetch Chrome versions: {error}")))?
        .error_for_status()
        .map_err(|error| {
            browser_error(format!("Chrome version metadata request failed: {error}"))
        })?;
    let body: ChromeVersions = resp
        .json()
        .await
        .map_err(|error| browser_error(format!("Failed to parse Chrome versions JSON: {error}")))?;
    let version = crate::install::validate_version_segment(&body.channels.stable.version)?;
    let download_url = body
        .channels
        .stable
        .downloads
        .chrome
        .iter()
        .find(|download| download.platform == platform)
        .map(|download| download.url.as_str())
        .ok_or_else(|| {
            browser_error(format!(
                "No Chrome download available for platform '{platform}'"
            ))
        })?;
    let download_url =
        crate::install::trusted_https_url(download_url, &["storage.googleapis.com"])?;

    let cache_dir = crate::chrome::managed_cache_dir()?;
    let _lock = crate::install::acquire_install_lock(&cache_dir).await?;
    crate::install::cleanup_stale_stages(&cache_dir).await?;
    let stage = crate::install::create_stage(&cache_dir, "chrome").await?;
    let archive_path = stage.join(".archive.zip");

    info!("Downloading Chrome for Testing v{version} ({platform})...");
    let artifact = crate::install::download_to_file(
        &client,
        download_url.clone(),
        &archive_path,
        MAX_CHROME_ARCHIVE_BYTES,
    )
    .await?;

    info!(
        "Downloaded {:.1} MB, extracting...",
        artifact.bytes as f64 / 1_048_576.0
    );
    let blocking_archive = archive_path.clone();
    let blocking_stage = stage.clone();
    tokio::task::spawn_blocking(move || extract_zip_file(&blocking_archive, &blocking_stage))
        .await
        .map_err(|error| browser_error(format!("Chrome extraction task failed: {error}")))??;
    tokio::fs::remove_file(&archive_path)
        .await
        .map_err(|error| {
            browser_error(format!(
                "Failed to remove staged Chrome archive '{}': {error}",
                archive_path.display()
            ))
        })?;

    let relative_executable = crate::chrome::chrome_executable_in_zip(platform);
    let staged_executable = stage.join(&relative_executable);
    make_executable(&staged_executable).await?;
    if !staged_executable.is_file() {
        let contents: Vec<_> = std::fs::read_dir(&stage)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.path())
                    .collect()
            })
            .unwrap_or_default();
        warn!(
            "Expected Chrome at {} but found {:?}",
            staged_executable.display(),
            contents
        );
        return Err(browser_error(format!(
            "Chrome executable not found after extraction at {}",
            staged_executable.display()
        )));
    }

    let executable_sha256 = crate::install::sha256_file(&staged_executable).await?;
    crate::install::write_receipt(
        &stage,
        &crate::install::ManagedInstallReceipt {
            schema_version: 1,
            provider: "chrome".to_string(),
            version: version.to_string(),
            source_url: download_url.to_string(),
            artifact_sha256: artifact.sha256,
            artifact_bytes: artifact.bytes,
            executable_sha256,
            integrity_policy: "approved-https-source+recorded-sha256".to_string(),
        },
    )
    .await?;

    let version_dir = cache_dir.join(version);
    crate::install::activate_directory(&stage, &version_dir).await?;
    let executable = version_dir.join(relative_executable);
    info!(
        "Chrome for Testing v{version} installed at {}",
        executable.display()
    );
    Ok(executable)
}

#[cfg(unix)]
async fn make_executable(path: &Path) -> UseResult<()> {
    if path.is_file() {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = tokio::fs::metadata(path)
            .await
            .map_err(|error| browser_error(format!("Failed to read Chrome permissions: {error}")))?
            .permissions();
        permissions.set_mode(0o755);
        tokio::fs::set_permissions(path, permissions)
            .await
            .map_err(|error| browser_error(format!("Failed to set Chrome permissions: {error}")))?;
    }
    Ok(())
}

#[cfg(not(unix))]
async fn make_executable(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn extract_zip_file(archive_path: &Path, target_dir: &Path) -> UseResult<()> {
    let archive_file = std::fs::File::open(archive_path).map_err(|error| {
        browser_error(format!(
            "Failed to open Chrome archive '{}': {error}",
            archive_path.display()
        ))
    })?;
    let mut archive = zip::ZipArchive::new(archive_file)
        .map_err(|error| browser_error(format!("Failed to open zip archive: {error}")))?;
    if archive.len() > MAX_CHROME_ARCHIVE_ENTRIES {
        return Err(browser_error(format!(
            "Chrome archive contains more than {MAX_CHROME_ARCHIVE_ENTRIES} entries."
        )));
    }
    let mut extracted_bytes = 0_u64;

    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .map_err(|error| browser_error(format!("Failed to read zip entry {index}: {error}")))?;
        extracted_bytes = extracted_bytes
            .checked_add(file.size())
            .ok_or_else(|| browser_error("Chrome archive expanded size overflowed."))?;
        if extracted_bytes > MAX_CHROME_EXTRACTED_BYTES {
            return Err(browser_error(format!(
                "Chrome archive expands beyond the {MAX_CHROME_EXTRACTED_BYTES}-byte limit."
            )));
        }

        let out_path = target_dir.join(file.mangled_name());
        if file.is_dir() {
            std::fs::create_dir_all(&out_path).map_err(|error| {
                browser_error(format!(
                    "Failed to create directory '{}': {error}",
                    out_path.display()
                ))
            })?;
            continue;
        }
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                browser_error(format!(
                    "Failed to create directory '{}': {error}",
                    parent.display()
                ))
            })?;
        }
        let mut output = std::fs::File::create(&out_path).map_err(|error| {
            browser_error(format!(
                "Failed to create file '{}': {error}",
                out_path.display()
            ))
        })?;
        std::io::copy(&mut file, &mut output).map_err(|error| {
            browser_error(format!(
                "Failed to extract file '{}': {error}",
                out_path.display()
            ))
        })?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = file.unix_mode() {
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(mode));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_zip_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("invalid.zip");
        std::fs::write(&archive, b"not a zip").unwrap();
        let error = extract_zip_file(&archive, temp.path()).unwrap_err();
        assert!(error.to_string().contains("zip"));
    }
}
