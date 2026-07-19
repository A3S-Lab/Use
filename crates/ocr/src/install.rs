use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use a3s_use_core::{FirstUseInstallPolicy, UseError, UseResult};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::assets::{
    managed_model_dir, managed_root, ocr_status, validate_assets, OcrInstallSource,
    OcrRuntimeStatus, RECEIPT_FILE,
};
use crate::config::MODEL_FAMILY;

const INSTALL_LOCK: &str = ".install.lock";
const STAGE_PREFIX: &str = ".stage-";
const BACKUP_PREFIX: &str = ".backup-";
const DOWNLOAD_HOST: &str = "paddle-model-ecology.bj.bcebos.com";
const MAX_ARCHIVE_BYTES: u64 = 256 * 1024 * 1024;

const DETECTION_ARCHIVE: PinnedArchive = PinnedArchive {
    role: "det",
    directory: "PP-OCRv6_small_det_onnx_infer",
    url: "https://paddle-model-ecology.bj.bcebos.com/paddlex/official_inference_model/paddle3.0.0/PP-OCRv6_small_det_onnx_infer.tar",
    bytes: 9_891_840,
    sha256: "d218f6fbf0f1c23d2161bd6ac7f5eaa6104fa89955c09290497e31008e2618e4",
};
const RECOGNITION_ARCHIVE: PinnedArchive = PinnedArchive {
    role: "rec",
    directory: "PP-OCRv6_small_rec_onnx_infer",
    url: "https://paddle-model-ecology.bj.bcebos.com/paddlex/official_inference_model/paddle3.0.0/PP-OCRv6_small_rec_onnx_infer.tar",
    bytes: 21_319_680,
    sha256: "d267ab077a44a0eedb1ea8f8c542d263f211de8e9d7a029bf9fcfff7e5a88fb1",
};

#[derive(Debug, Clone, Copy)]
struct PinnedArchive {
    role: &'static str,
    directory: &'static str,
    url: &'static str,
    bytes: u64,
    sha256: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct InstallReceipt {
    schema_version: u32,
    provider: String,
    model: String,
    detection_url: String,
    detection_sha256: String,
    recognition_url: String,
    recognition_sha256: String,
}

struct InstallLock {
    _file: std::fs::File,
}

struct Downloaded {
    bytes: u64,
    sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutoInstallAction {
    Ready,
    Install,
}

/// Ensure the pinned PP-OCRv6 bundle is ready for an actual OCR operation.
///
/// Read-only diagnostics deliberately do not call this function. Direct OCR
/// extraction and the bounded MCP install tool use it so first use installs or
/// repairs A3S-managed models while preserving offline, no-auto-install, and
/// explicit-model-directory boundaries.
pub async fn ensure_ppocr_v6_ready() -> UseResult<OcrRuntimeStatus> {
    let status = ocr_status();
    match automatic_install_action(&status, FirstUseInstallPolicy::from_env()?)? {
        AutoInstallAction::Ready => Ok(status),
        AutoInstallAction::Install => install_ppocr_v6(false).await,
    }
}

pub async fn install_ppocr_v6(force: bool) -> UseResult<OcrRuntimeStatus> {
    let current = ocr_status();
    if !force && current.available {
        return Ok(current);
    }

    let root = managed_root()?;
    let _lock = acquire_lock(&root).await?;
    cleanup_stale(&root).await?;

    let current = ocr_status();
    if !force && current.available {
        return Ok(current);
    }

    let stage = create_stage(&root).await?;
    let install_result = install_into(&stage).await;
    if install_result.is_err() {
        let _ = tokio::fs::remove_dir_all(&stage).await;
    }
    install_result?;

    let target = managed_model_dir()?;
    activate(&stage, &target).await?;
    validate_assets(&target, OcrInstallSource::Managed)?;

    let status = ocr_status();
    if status.available {
        Ok(status)
    } else {
        Err(ocr_error(
            "use.ocr.install_failed",
            "PP-OCRv6 installation completed without a usable model bundle.",
        ))
    }
}

pub async fn repair_ppocr_v6() -> UseResult<OcrRuntimeStatus> {
    let status = ocr_status();
    if status.available {
        Ok(status)
    } else {
        install_ppocr_v6(true).await
    }
}

pub async fn uninstall_managed_ppocr_v6() -> UseResult<bool> {
    let root = managed_root()?;
    let _lock = acquire_lock(&root).await?;
    let target = managed_model_dir()?;
    if !owned_install(&target) {
        return Ok(false);
    }
    tokio::fs::remove_dir_all(&target).await.map_err(|error| {
        ocr_error(
            "use.ocr.uninstall_failed",
            format!(
                "Failed to remove managed PP-OCRv6 bundle '{}': {error}",
                target.display()
            ),
        )
    })?;
    Ok(true)
}

async fn install_into(stage: &Path) -> UseResult<()> {
    let client = download_client()?;
    for archive in [DETECTION_ARCHIVE, RECOGNITION_ARCHIVE] {
        let archive_path = stage.join(format!("{}.tar", archive.role));
        let downloaded = download(&client, archive.url, &archive_path).await?;
        if downloaded.bytes != archive.bytes || downloaded.sha256 != archive.sha256 {
            return Err(ocr_error(
                "use.ocr.integrity_mismatch",
                format!(
                    "{} archive integrity mismatch: expected {} bytes and {}, got {} bytes and {}.",
                    archive.directory,
                    archive.bytes,
                    archive.sha256,
                    downloaded.bytes,
                    downloaded.sha256
                ),
            ));
        }
        let archive_path_for_task = archive_path.clone();
        let destination = stage.join(archive.role);
        tokio::task::spawn_blocking(move || {
            extract_archive(&archive_path_for_task, &destination, archive)
        })
        .await
        .map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!("PP-OCRv6 archive extraction task failed: {error}"),
            )
        })??;
        tokio::fs::remove_file(&archive_path)
            .await
            .map_err(|error| {
                ocr_error(
                    "use.ocr.install_failed",
                    format!(
                        "Failed to remove staged archive '{}': {error}",
                        archive_path.display()
                    ),
                )
            })?;
    }
    write_receipt(stage).await?;
    validate_assets(stage, OcrInstallSource::Managed)?;
    Ok(())
}

fn download_client() -> UseResult<reqwest::Client> {
    let redirects = reqwest::redirect::Policy::custom(|attempt| {
        let approved = attempt.previous().len() < 5
            && attempt.url().scheme() == "https"
            && attempt.url().host_str() == Some(DOWNLOAD_HOST);
        if approved {
            attempt.follow()
        } else {
            attempt.error("PP-OCRv6 download redirected to an unapproved host")
        }
    });
    reqwest::Client::builder()
        .user_agent(concat!("a3s-use-ocr/", env!("CARGO_PKG_VERSION")))
        .redirect(redirects)
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|error| {
            ocr_error(
                "use.ocr.download_failed",
                format!("Failed to create PP-OCRv6 download client: {error}"),
            )
        })
}

async fn download(
    client: &reqwest::Client,
    value: &str,
    destination: &Path,
) -> UseResult<Downloaded> {
    let url = reqwest::Url::parse(value).map_err(|error| {
        ocr_error(
            "use.ocr.download_source_invalid",
            format!("Invalid PP-OCRv6 download URL: {error}"),
        )
    })?;
    if url.scheme() != "https" || url.host_str() != Some(DOWNLOAD_HOST) {
        return Err(ocr_error(
            "use.ocr.download_source_invalid",
            "PP-OCRv6 download source is not the pinned official HTTPS host.",
        ));
    }
    let mut response = client
        .get(url)
        .send()
        .await
        .map_err(|error| {
            ocr_error(
                "use.ocr.download_failed",
                format!("Failed to download PP-OCRv6: {error}"),
            )
        })?
        .error_for_status()
        .map_err(|error| {
            ocr_error(
                "use.ocr.download_failed",
                format!("PP-OCRv6 download failed: {error}"),
            )
        })?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_ARCHIVE_BYTES)
    {
        return Err(ocr_error(
            "use.ocr.download_too_large",
            "PP-OCRv6 archive exceeds the 256 MiB limit.",
        ));
    }
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .await
        .map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!(
                    "Failed to create PP-OCRv6 download '{}': {error}",
                    destination.display()
                ),
            )
        })?;
    let mut hasher = Sha256::new();
    let mut total = 0_u64;
    while let Some(chunk) = response.chunk().await.map_err(|error| {
        ocr_error(
            "use.ocr.download_failed",
            format!("Failed to read PP-OCRv6 download: {error}"),
        )
    })? {
        total = total
            .checked_add(chunk.len() as u64)
            .ok_or_else(|| ocr_error("use.ocr.download_too_large", "Download size overflowed."))?;
        if total > MAX_ARCHIVE_BYTES {
            return Err(ocr_error(
                "use.ocr.download_too_large",
                "PP-OCRv6 archive exceeds the 256 MiB limit.",
            ));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!(
                    "Failed to write PP-OCRv6 download '{}': {error}",
                    destination.display()
                ),
            )
        })?;
    }
    file.flush().await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to flush PP-OCRv6 download '{}': {error}",
                destination.display()
            ),
        )
    })?;
    file.sync_all().await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to sync PP-OCRv6 download '{}': {error}",
                destination.display()
            ),
        )
    })?;
    Ok(Downloaded {
        bytes: total,
        sha256: format!("{:x}", hasher.finalize()),
    })
}

fn extract_archive(archive_path: &Path, destination: &Path, spec: PinnedArchive) -> UseResult<()> {
    std::fs::create_dir(destination).map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to create PP-OCRv6 model directory '{}': {error}",
                destination.display()
            ),
        )
    })?;
    let file = std::fs::File::open(archive_path).map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to open PP-OCRv6 archive '{}': {error}",
                archive_path.display()
            ),
        )
    })?;
    let mut archive = tar::Archive::new(file);
    let mut extracted = [false; 2];
    for entry in archive.entries().map_err(archive_error)? {
        let entry = entry.map_err(archive_error)?;
        let path = entry.path().map_err(archive_error)?;
        let components = path.components().collect::<Vec<_>>();
        if components.len() == 1
            && matches!(components[0], Component::Normal(value) if value == spec.directory)
            && entry.header().entry_type().is_dir()
        {
            continue;
        }
        if components.len() != 2
            || !matches!(components[0], Component::Normal(value) if value == spec.directory)
            || !entry.header().entry_type().is_file()
        {
            return Err(ocr_error(
                "use.ocr.archive_invalid",
                format!(
                    "PP-OCRv6 archive contains an unexpected entry '{}'.",
                    path.display()
                ),
            ));
        }
        let name = match components[1] {
            Component::Normal(name) if name == "inference.onnx" => {
                extracted[0] = true;
                "inference.onnx"
            }
            Component::Normal(name) if name == "inference.yml" => {
                extracted[1] = true;
                "inference.yml"
            }
            _ => {
                return Err(ocr_error(
                    "use.ocr.archive_invalid",
                    format!(
                        "PP-OCRv6 archive contains an unexpected entry '{}'.",
                        path.display()
                    ),
                ))
            }
        };
        let max = if name.ends_with(".onnx") {
            256 * 1024 * 1024
        } else {
            2 * 1024 * 1024
        };
        if entry.size() == 0 || entry.size() > max {
            return Err(ocr_error(
                "use.ocr.archive_invalid",
                format!("PP-OCRv6 archive entry '{name}' has an invalid size."),
            ));
        }
        let expected_size = entry.size();
        let output_path = destination.join(name);
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&output_path)
            .map_err(|error| {
                ocr_error(
                    "use.ocr.install_failed",
                    format!(
                        "Failed to create PP-OCRv6 asset '{}': {error}",
                        output_path.display()
                    ),
                )
            })?;
        let copied = std::io::copy(&mut entry.take(max + 1), &mut output).map_err(archive_error)?;
        if copied != expected_size {
            return Err(ocr_error(
                "use.ocr.archive_invalid",
                format!("PP-OCRv6 archive entry '{name}' was truncated."),
            ));
        }
        output.flush().map_err(archive_error)?;
        output.sync_all().map_err(archive_error)?;
    }
    if !extracted.into_iter().all(|present| present) {
        return Err(ocr_error(
            "use.ocr.archive_invalid",
            "PP-OCRv6 archive is missing inference.onnx or inference.yml.",
        ));
    }
    Ok(())
}

async fn write_receipt(stage: &Path) -> UseResult<()> {
    let receipt = InstallReceipt {
        schema_version: 1,
        provider: "pp-ocr-v6".to_string(),
        model: MODEL_FAMILY.to_string(),
        detection_url: DETECTION_ARCHIVE.url.to_string(),
        detection_sha256: DETECTION_ARCHIVE.sha256.to_string(),
        recognition_url: RECOGNITION_ARCHIVE.url.to_string(),
        recognition_sha256: RECOGNITION_ARCHIVE.sha256.to_string(),
    };
    let bytes = serde_json::to_vec_pretty(&receipt).map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!("Failed to encode PP-OCRv6 install receipt: {error}"),
        )
    })?;
    let path = stage.join(RECEIPT_FILE);
    let mut file = tokio::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&path)
        .await
        .map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!(
                    "Failed to create PP-OCRv6 receipt '{}': {error}",
                    path.display()
                ),
            )
        })?;
    file.write_all(&bytes).await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to write PP-OCRv6 receipt '{}': {error}",
                path.display()
            ),
        )
    })?;
    file.sync_all().await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to sync PP-OCRv6 receipt '{}': {error}",
                path.display()
            ),
        )
    })
}

async fn acquire_lock(root: &Path) -> UseResult<InstallLock> {
    tokio::fs::create_dir_all(root).await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to create OCR data root '{}': {error}",
                root.display()
            ),
        )
    })?;
    let path = root.join(INSTALL_LOCK);
    tokio::task::spawn_blocking(move || {
        let file = OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&path)
            .map_err(|error| {
                ocr_error(
                    "use.ocr.install_failed",
                    format!(
                        "Failed to open OCR install lock '{}': {error}",
                        path.display()
                    ),
                )
            })?;
        file.lock_exclusive().map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!(
                    "Failed to acquire OCR install lock '{}': {error}",
                    path.display()
                ),
            )
        })?;
        Ok(InstallLock { _file: file })
    })
    .await
    .map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!("OCR install lock task failed: {error}"),
        )
    })?
}

async fn create_stage(root: &Path) -> UseResult<PathBuf> {
    static NEXT_STAGE: AtomicU64 = AtomicU64::new(1);
    for _ in 0..32 {
        let path = root.join(format!(
            "{STAGE_PREFIX}{}-{}",
            std::process::id(),
            NEXT_STAGE.fetch_add(1, Ordering::Relaxed)
        ));
        match tokio::fs::create_dir(&path).await {
            Ok(()) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(ocr_error(
                    "use.ocr.install_failed",
                    format!(
                        "Failed to create OCR staging directory '{}': {error}",
                        path.display()
                    ),
                ))
            }
        }
    }
    Err(ocr_error(
        "use.ocr.install_failed",
        "Failed to allocate a unique OCR staging directory.",
    ))
}

async fn cleanup_stale(root: &Path) -> UseResult<()> {
    let mut entries = tokio::fs::read_dir(root).await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to inspect OCR data root '{}': {error}",
                root.display()
            ),
        )
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to inspect OCR data root '{}': {error}",
                root.display()
            ),
        )
    })? {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let is_owned_backup = name.starts_with(BACKUP_PREFIX) && owned_install(&entry.path());
        if name.starts_with(STAGE_PREFIX) || is_owned_backup {
            tokio::fs::remove_dir_all(entry.path())
                .await
                .map_err(|error| {
                    ocr_error(
                        "use.ocr.install_failed",
                        format!("Failed to remove stale OCR staging directory: {error}"),
                    )
                })?;
        }
    }
    Ok(())
}

async fn activate(stage: &Path, target: &Path) -> UseResult<()> {
    static NEXT_BACKUP: AtomicU64 = AtomicU64::new(1);
    let parent = target.parent().ok_or_else(|| {
        ocr_error(
            "use.ocr.install_failed",
            "OCR install target has no parent directory.",
        )
    })?;
    let backup = parent.join(format!(
        "{BACKUP_PREFIX}{}-{}",
        std::process::id(),
        NEXT_BACKUP.fetch_add(1, Ordering::Relaxed)
    ));
    let had_target = tokio::fs::try_exists(target).await.map_err(|error| {
        ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to inspect OCR install target '{}': {error}",
                target.display()
            ),
        )
    })?;
    if had_target {
        if !owned_install(target) {
            return Err(ocr_error(
                "use.ocr.install_target_unowned",
                format!(
                    "Refusing to replace unowned OCR model directory '{}'.",
                    target.display()
                ),
            ));
        }
        tokio::fs::rename(target, &backup).await.map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!(
                    "Failed to stage existing OCR install '{}': {error}",
                    target.display()
                ),
            )
        })?;
    }
    if let Err(error) = tokio::fs::rename(stage, target).await {
        if had_target {
            let _ = tokio::fs::rename(&backup, target).await;
        }
        return Err(ocr_error(
            "use.ocr.install_failed",
            format!(
                "Failed to activate OCR install '{}': {error}",
                target.display()
            ),
        ));
    }
    if had_target {
        tokio::fs::remove_dir_all(&backup).await.map_err(|error| {
            ocr_error(
                "use.ocr.install_failed",
                format!(
                    "Activated OCR but failed to remove backup '{}': {error}",
                    backup.display()
                ),
            )
        })?;
    }
    Ok(())
}

fn owned_install(path: &Path) -> bool {
    let Ok(bytes) = std::fs::read(path.join(RECEIPT_FILE)) else {
        return false;
    };
    serde_json::from_slice::<InstallReceipt>(&bytes).is_ok_and(|receipt| {
        receipt.schema_version == 1
            && receipt.provider == "pp-ocr-v6"
            && receipt.model == MODEL_FAMILY
    })
}

fn automatic_install_action(
    status: &OcrRuntimeStatus,
    policy: FirstUseInstallPolicy,
) -> UseResult<AutoInstallAction> {
    if status.available {
        return Ok(AutoInstallAction::Ready);
    }
    if status.source == OcrInstallSource::Environment {
        return Err(ocr_error(
            "use.ocr.model_unreadable",
            format!(
                "The explicit A3S_OCR_MODEL_DIR is not usable: {}",
                status.detail
            ),
        )
        .with_suggestion("Fix or unset A3S_OCR_MODEL_DIR before retrying OCR."));
    }
    if let Some(block) = policy.blocked_by() {
        let reason = block.reason();
        return Err(ocr_error(
            "use.ocr.auto_install_disabled",
            format!(
                "The local {MODEL_FAMILY} bundle is not ready and automatic installation is disabled by {reason}."
            ),
        )
        .with_suggestion(
            "Enable first-use installation or run 'a3s install use/ocr' explicitly while online.",
        )
        .with_detail("reason", reason));
    }
    Ok(AutoInstallAction::Install)
}

fn archive_error(error: impl std::fmt::Display) -> UseError {
    ocr_error(
        "use.ocr.archive_invalid",
        format!("Failed to extract PP-OCRv6 archive: {error}"),
    )
}

fn ocr_error(code: &str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod automatic_install_tests {
    use super::*;

    fn status(available: bool, source: OcrInstallSource) -> OcrRuntimeStatus {
        OcrRuntimeStatus {
            available,
            source,
            model: MODEL_FAMILY.to_string(),
            model_dir: None,
            managed_root: None,
            detail: if available {
                "ready".to_string()
            } else {
                "missing".to_string()
            },
        }
    }

    #[test]
    fn ready_models_never_require_an_install() {
        let action = automatic_install_action(
            &status(true, OcrInstallSource::Managed),
            FirstUseInstallPolicy::new(true, true),
        )
        .unwrap();

        assert_eq!(action, AutoInstallAction::Ready);
    }

    #[test]
    fn missing_models_install_when_first_use_mutation_is_allowed() {
        let action = automatic_install_action(
            &status(false, OcrInstallSource::Missing),
            FirstUseInstallPolicy::new(false, false),
        )
        .unwrap();

        assert_eq!(action, AutoInstallAction::Install);
    }

    #[test]
    fn offline_and_no_auto_install_are_strict_boundaries() {
        for policy in [
            FirstUseInstallPolicy::new(true, false),
            FirstUseInstallPolicy::new(false, true),
        ] {
            let error = automatic_install_action(&status(false, OcrInstallSource::Missing), policy)
                .unwrap_err();
            assert_eq!(error.code, "use.ocr.auto_install_disabled");
        }
    }

    #[test]
    fn an_invalid_explicit_model_directory_is_never_replaced_implicitly() {
        let error = automatic_install_action(
            &status(false, OcrInstallSource::Environment),
            FirstUseInstallPolicy::new(false, false),
        )
        .unwrap_err();

        assert_eq!(error.code, "use.ocr.model_unreadable");
    }
}
