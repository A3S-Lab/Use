//! Shared managed-runtime installation primitives.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::warn;
use url::Url;

use a3s_use_core::UseResult;

use crate::pool::browser_error;

pub(crate) const RECEIPT_FILE: &str = ".a3s-install.json";
const INSTALL_LOCK_FILE: &str = ".install.lock";
const STAGE_PREFIX: &str = ".a3s-stage-";
const BACKUP_PREFIX: &str = ".a3s-backup-";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ManagedInstallReceipt {
    pub schema_version: u32,
    pub provider: String,
    pub version: String,
    pub source_url: String,
    pub artifact_sha256: String,
    pub artifact_bytes: u64,
    pub executable_sha256: String,
    pub integrity_policy: String,
}

pub(crate) struct DownloadedArtifact {
    pub sha256: String,
    pub bytes: u64,
}

/// Process-wide lock guard. The operating system releases the lock on drop.
pub(crate) struct InstallLock {
    _file: std::fs::File,
}

pub(crate) async fn acquire_install_lock(root: &Path) -> UseResult<InstallLock> {
    tokio::fs::create_dir_all(root).await.map_err(|error| {
        browser_error(format!(
            "Failed to create managed Browser directory '{}': {error}",
            root.display()
        ))
    })?;
    let lock_path = root.join(INSTALL_LOCK_FILE);
    tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)
            .map_err(|error| {
                browser_error(format!(
                    "Failed to open Browser install lock '{}': {error}",
                    lock_path.display()
                ))
            })?;
        file.lock_exclusive().map_err(|error| {
            browser_error(format!(
                "Failed to acquire Browser install lock '{}': {error}",
                lock_path.display()
            ))
        })?;
        Ok(InstallLock { _file: file })
    })
    .await
    .map_err(|error| browser_error(format!("Browser install lock task failed: {error}")))?
}

pub(crate) fn trusted_https_url(value: &str, allowed_hosts: &[&str]) -> UseResult<Url> {
    let url = Url::parse(value)
        .map_err(|error| browser_error(format!("Invalid Browser download URL: {error}")))?;
    let host = url
        .host_str()
        .ok_or_else(|| browser_error("Browser download URL has no host."))?;
    if url.scheme() != "https" || !allowed_hosts.contains(&host) {
        return Err(browser_error(format!(
            "Browser download source '{}' is not an approved HTTPS host.",
            url
        )));
    }
    Ok(url)
}

pub(crate) fn approved_redirect_policy(allowed_hosts: &[&str]) -> reqwest::redirect::Policy {
    let allowed_hosts: Vec<String> = allowed_hosts
        .iter()
        .map(|host| (*host).to_string())
        .collect();
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("too many Browser download redirects");
        }
        let approved = attempt.url().scheme() == "https"
            && attempt
                .url()
                .host_str()
                .is_some_and(|host| allowed_hosts.iter().any(|allowed| allowed == host));
        if approved {
            attempt.follow()
        } else {
            let url = attempt.url().to_string();
            attempt.error(format!(
                "Browser download redirected to unapproved URL '{url}'"
            ))
        }
    })
}

pub(crate) fn validate_version_segment(value: &str) -> UseResult<&str> {
    let valid = !value.is_empty()
        && value != "."
        && value != ".."
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if !valid {
        return Err(browser_error(format!(
            "Browser provider returned an invalid version identifier '{value}'."
        )));
    }
    Ok(value)
}

#[cfg(any(feature = "lightpanda", test))]
pub(crate) fn parse_published_sha256(value: &str) -> UseResult<String> {
    let digest = value.strip_prefix("sha256:").ok_or_else(|| {
        browser_error("Browser publisher digest does not use the sha256 algorithm.")
    })?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(browser_error(
            "Browser publisher returned an invalid SHA-256 digest.",
        ));
    }
    Ok(digest.to_ascii_lowercase())
}

pub(crate) async fn create_stage(root: &Path, label: &str) -> UseResult<PathBuf> {
    static NEXT_STAGE_ID: AtomicU64 = AtomicU64::new(1);
    for _ in 0..32 {
        let id = NEXT_STAGE_ID.fetch_add(1, Ordering::Relaxed);
        let path = root.join(format!("{STAGE_PREFIX}{label}-{}-{id}", std::process::id()));
        match tokio::fs::create_dir(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(browser_error(format!(
                    "Failed to create Browser staging directory '{}': {error}",
                    path.display()
                )))
            }
        }
    }
    Err(browser_error(
        "Failed to allocate a unique Browser staging directory.",
    ))
}

pub(crate) async fn cleanup_stale_stages(root: &Path) -> UseResult<()> {
    let mut entries = tokio::fs::read_dir(root).await.map_err(|error| {
        browser_error(format!(
            "Failed to inspect Browser install directory '{}': {error}",
            root.display()
        ))
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        browser_error(format!(
            "Failed to inspect Browser install entry in '{}': {error}",
            root.display()
        ))
    })? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(STAGE_PREFIX) {
            let path = entry.path();
            let file_type = entry.file_type().await.map_err(|error| {
                browser_error(format!(
                    "Failed to inspect stale Browser path '{}': {error}",
                    path.display()
                ))
            })?;
            let result = if file_type.is_dir() {
                tokio::fs::remove_dir_all(&path).await
            } else {
                tokio::fs::remove_file(&path).await
            };
            if let Err(error) = result {
                return Err(browser_error(format!(
                    "Failed to remove stale Browser staging path '{}': {error}",
                    path.display()
                )));
            }
        } else if name.starts_with(BACKUP_PREFIX) {
            let path = entry.path();
            let Some(receipt) = load_receipt(&path) else {
                return Err(browser_error(format!(
                    "Cannot safely recover Browser install backup '{}': its receipt is missing or invalid.",
                    path.display()
                )));
            };
            validate_version_segment(&receipt.version)?;
            let target = root.join(&receipt.version);
            if tokio::fs::try_exists(&target).await.map_err(|error| {
                browser_error(format!(
                    "Failed to inspect Browser recovery target '{}': {error}",
                    target.display()
                ))
            })? {
                tokio::fs::remove_dir_all(&path).await.map_err(|error| {
                    browser_error(format!(
                        "Failed to remove stale Browser backup '{}': {error}",
                        path.display()
                    ))
                })?;
            } else {
                tokio::fs::rename(&path, &target).await.map_err(|error| {
                    browser_error(format!(
                        "Failed to recover Browser backup '{}' to '{}': {error}",
                        path.display(),
                        target.display()
                    ))
                })?;
            }
        }
    }
    Ok(())
}

pub(crate) async fn download_to_file(
    client: &reqwest::Client,
    url: Url,
    destination: &Path,
    max_bytes: u64,
) -> UseResult<DownloadedArtifact> {
    let mut response = client
        .get(url.clone())
        .send()
        .await
        .map_err(|error| browser_error(format!("Failed to download Browser runtime: {error}")))?
        .error_for_status()
        .map_err(|error| browser_error(format!("Browser runtime download failed: {error}")))?;
    if response
        .content_length()
        .is_some_and(|length| length > max_bytes)
    {
        return Err(browser_error(format!(
            "Browser runtime download exceeds the {max_bytes}-byte limit."
        )));
    }

    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await
        .map_err(|error| {
            browser_error(format!(
                "Failed to create Browser download '{}': {error}",
                destination.display()
            ))
        })?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        browser_error(format!("Failed to read Browser runtime download: {error}"))
    })? {
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| browser_error("Browser runtime download size overflowed."))?;
        if total > max_bytes {
            return Err(browser_error(format!(
                "Browser runtime download exceeds the {max_bytes}-byte limit."
            )));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|error| {
            browser_error(format!(
                "Failed to write Browser download '{}': {error}",
                destination.display()
            ))
        })?;
    }
    file.flush().await.map_err(|error| {
        browser_error(format!(
            "Failed to flush Browser download '{}': {error}",
            destination.display()
        ))
    })?;
    file.sync_all().await.map_err(|error| {
        browser_error(format!(
            "Failed to sync Browser download '{}': {error}",
            destination.display()
        ))
    })?;

    Ok(DownloadedArtifact {
        sha256: format!("{:x}", hasher.finalize()),
        bytes: total,
    })
}

pub(crate) async fn sha256_file(path: &Path) -> UseResult<String> {
    let mut file = tokio::fs::File::open(path).await.map_err(|error| {
        browser_error(format!(
            "Failed to open Browser executable '{}': {error}",
            path.display()
        ))
    })?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = file.read(&mut buffer).await.map_err(|error| {
            browser_error(format!(
                "Failed to hash Browser executable '{}': {error}",
                path.display()
            ))
        })?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

pub(crate) async fn write_receipt(
    install_dir: &Path,
    receipt: &ManagedInstallReceipt,
) -> UseResult<()> {
    let bytes = serde_json::to_vec_pretty(receipt)
        .map_err(|error| browser_error(format!("Failed to encode install receipt: {error}")))?;
    let path = install_dir.join(RECEIPT_FILE);
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
        .map_err(|error| {
            browser_error(format!(
                "Failed to create Browser install receipt '{}': {error}",
                path.display()
            ))
        })?;
    file.write_all(&bytes).await.map_err(|error| {
        browser_error(format!(
            "Failed to write Browser install receipt '{}': {error}",
            path.display()
        ))
    })?;
    file.sync_all().await.map_err(|error| {
        browser_error(format!(
            "Failed to sync Browser install receipt '{}': {error}",
            path.display()
        ))
    })
}

pub(crate) fn has_complete_receipt(install_dir: &Path, provider: &str, version: &str) -> bool {
    load_receipt(install_dir).is_some_and(|receipt| {
        receipt.schema_version == 1
            && receipt.provider == provider
            && receipt.version == version
            && receipt.artifact_sha256.len() == 64
            && receipt.executable_sha256.len() == 64
    })
}

fn load_receipt(install_dir: &Path) -> Option<ManagedInstallReceipt> {
    let bytes = std::fs::read(install_dir.join(RECEIPT_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

pub(crate) async fn activate_directory(stage: &Path, target: &Path) -> UseResult<()> {
    let Some(parent) = target.parent() else {
        return Err(browser_error(
            "Browser install target has no parent directory.",
        ));
    };
    let backup = parent.join(format!(
        "{BACKUP_PREFIX}{}-{}-{}",
        target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("runtime"),
        std::process::id(),
        NEXT_ACTIVATION_ID.fetch_add(1, Ordering::Relaxed)
    ));

    let had_target = tokio::fs::try_exists(target).await.map_err(|error| {
        browser_error(format!(
            "Failed to inspect Browser install target '{}': {error}",
            target.display()
        ))
    })?;
    if had_target {
        tokio::fs::rename(target, &backup).await.map_err(|error| {
            browser_error(format!(
                "Failed to stage existing Browser install '{}': {error}",
                target.display()
            ))
        })?;
    }

    if let Err(error) = tokio::fs::rename(stage, target).await {
        if had_target {
            let _ = tokio::fs::rename(&backup, target).await;
        }
        return Err(browser_error(format!(
            "Failed to activate Browser install '{}': {error}",
            target.display()
        )));
    }
    if had_target {
        if let Err(error) = tokio::fs::remove_dir_all(&backup).await {
            warn!(
                "Activated Browser install but failed to remove backup '{}': {error}",
                backup.display()
            );
        }
    }
    Ok(())
}

static NEXT_ACTIVATION_ID: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
#[path = "install_tests.rs"]
mod tests;
