use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseError, UseResult};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncReadExt;

use super::storage::validate_directory_chain;
use super::{
    ActiveStateRestoreMarker, StateRestoreOperation, StateRestoreOperationStatus, MAX_MARKER_BYTES,
    MAX_OPERATION_BYTES, MAX_OPERATION_COUNT,
};

const EXCLUDED_ACTIVE_DOMAIN: &[u8] = b"a3s.use.state-restore-history-excluded-active.v1\0";

pub(crate) const STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES: u64 = MAX_OPERATION_BYTES;
pub(crate) const STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES: u64 =
    MAX_OPERATION_COUNT as u64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateRestoreHistorySnapshotEntry {
    pub(crate) source: PathBuf,
    pub(crate) plan_digest: String,
    pub(crate) length: u64,
    pub(crate) sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct StateRestoreHistorySnapshotScan {
    pub(crate) terminal: Vec<StateRestoreHistorySnapshotEntry>,
    pub(crate) excluded_active_files: u64,
    pub(crate) excluded_active_inventory_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ExcludedActiveKind {
    Marker,
    Operation,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
struct ExcludedActiveFile {
    kind: ExcludedActiveKind,
    plan_digest: String,
    length: u64,
    sha256: String,
}

pub(crate) async fn scan_state_restore_history_snapshot(
    state_root: &Path,
    installation: &InstallationId,
) -> UseResult<StateRestoreHistorySnapshotScan> {
    installation.validate().map_err(wrap_invalid)?;
    validate_owned_directory(state_root, "state root").await?;
    let marker = read_active_marker(state_root).await?;
    let mut excluded = Vec::new();
    if let Some((marker, length, sha256)) = &marker {
        excluded.push(ExcludedActiveFile {
            kind: ExcludedActiveKind::Marker,
            plan_digest: marker.plan_digest.clone(),
            length: *length,
            sha256: sha256.clone(),
        });
    }

    let operations = state_root.join("operations");
    let root = operations.join("state-restores");
    if !optional_owned_directory(&operations, "operations root").await? {
        return finish_scan(Vec::new(), excluded);
    }
    if !optional_owned_directory(&root, "restore history root").await? {
        return finish_scan(Vec::new(), excluded);
    }
    validate_directory_chain(state_root, &root)
        .await
        .map_err(wrap_invalid)?;

    let mut directories = Vec::new();
    let mut reader = fs::read_dir(&root)
        .await
        .map_err(|error| snapshot_io("read restore history root", error))?;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| snapshot_io("read restore history entry", error))?
    {
        if directories.len() >= MAX_OPERATION_COUNT {
            return Err(snapshot_invalid(
                "The restore history exceeds its native retention bound.",
            ));
        }
        let segment = entry.file_name().into_string().map_err(|_| {
            snapshot_invalid("A restore history directory name is not valid UTF-8.")
        })?;
        if !valid_segment(&segment) {
            return Err(snapshot_invalid(
                "A restore history directory is not a canonical operation identity.",
            ));
        }
        let directory = entry.path();
        validate_owned_directory(&directory, "restore operation directory").await?;
        directories.push((segment, directory));
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));

    let mut terminal = Vec::new();
    for (segment, directory) in directories {
        let path = validate_operation_directory(&directory).await?;
        let bytes = read_bounded_file(&path, MAX_OPERATION_BYTES, "restore operation").await?;
        let plan_digest = format!("sha256:{segment}");
        let operation = decode_operation(&plan_digest, &bytes, installation)?;
        let length = bytes.len() as u64;
        let sha256 = digest(&bytes);
        if let Some((active_marker, _, _)) = marker
            .as_ref()
            .filter(|(marker, _, _)| marker.plan_digest == plan_digest)
        {
            if !active_marker
                .binds_operation(&operation)
                .map_err(wrap_invalid)?
            {
                return Err(snapshot_invalid(
                    "The active restore marker does not bind its retained operation.",
                ));
            }
            excluded.push(ExcludedActiveFile {
                kind: ExcludedActiveKind::Operation,
                plan_digest,
                length,
                sha256,
            });
        } else if operation.status == StateRestoreOperationStatus::Completed {
            terminal.push(StateRestoreHistorySnapshotEntry {
                source: path,
                plan_digest,
                length,
                sha256,
            });
        } else {
            return Err(snapshot_invalid(
                "A nonterminal restore operation has no exact active marker.",
            ));
        }
    }
    finish_scan(terminal, excluded)
}

pub(crate) async fn read_state_restore_history_snapshot_entry(
    entry: &StateRestoreHistorySnapshotEntry,
    installation: &InstallationId,
) -> UseResult<Vec<u8>> {
    let bytes = read_bounded_file(&entry.source, entry.length, "restore history record").await?;
    if bytes.len() as u64 != entry.length || digest(&bytes) != entry.sha256 {
        return Err(snapshot_invalid(
            "A restore history record changed while its snapshot was created.",
        ));
    }
    validate_terminal_state_restore_history_record(&entry.plan_digest, &bytes, installation)?;
    Ok(bytes)
}

pub(crate) fn validate_terminal_state_restore_history_record(
    plan_digest: &str,
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<()> {
    let operation = decode_operation(plan_digest, bytes, installation)?;
    if operation.status != StateRestoreOperationStatus::Completed {
        return Err(snapshot_invalid(
            "A restore history archive contains a nonterminal operation.",
        ));
    }
    Ok(())
}

fn decode_operation(
    plan_digest: &str,
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<StateRestoreOperation> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_OPERATION_BYTES {
        return Err(snapshot_invalid(
            "A restore history operation exceeds its native byte bound.",
        ));
    }
    let operation: StateRestoreOperation = serde_json::from_slice(bytes)
        .map_err(|_| snapshot_invalid("A restore history operation is invalid JSON."))?;
    operation.validate().map_err(wrap_invalid)?;
    let canonical = serde_json::to_vec(&operation)
        .map_err(|error| snapshot_invalid(format!("Failed to encode restore history: {error}")))?;
    if canonical != bytes
        || operation.plan_digest != plan_digest
        || operation.plan.backup.installation != *installation
    {
        return Err(snapshot_invalid(
            "A restore history operation is noncanonical, foreign, or rebound.",
        ));
    }
    Ok(operation)
}

fn finish_scan(
    terminal: Vec<StateRestoreHistorySnapshotEntry>,
    mut excluded: Vec<ExcludedActiveFile>,
) -> UseResult<StateRestoreHistorySnapshotScan> {
    excluded.sort();
    let excluded_active_files = u64::try_from(excluded.len())
        .map_err(|_| snapshot_invalid("Excluded restore history accounting overflowed."))?;
    let bytes = super::super::canonical_json(&excluded, "excluded active restore inventory")
        .map_err(wrap_invalid)?;
    let mut hasher = Sha256::new();
    hasher.update(EXCLUDED_ACTIVE_DOMAIN);
    hasher.update(bytes);
    Ok(StateRestoreHistorySnapshotScan {
        terminal,
        excluded_active_files,
        excluded_active_inventory_digest: format!("sha256:{:x}", hasher.finalize()),
    })
}

async fn read_active_marker(
    state_root: &Path,
) -> UseResult<Option<(ActiveStateRestoreMarker, u64, String)>> {
    let path = state_root.join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER);
    let temporary = state_root.join(format!(
        "{}.tmp",
        a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER
    ));
    reject_existing_path(&temporary, "temporary active restore marker").await?;
    let Some(bytes) =
        read_optional_bounded_file(&path, MAX_MARKER_BYTES, "active restore marker").await?
    else {
        return Ok(None);
    };
    let marker: ActiveStateRestoreMarker = serde_json::from_slice(&bytes)
        .map_err(|_| snapshot_invalid("The active restore marker is invalid JSON."))?;
    marker.validate().map_err(wrap_invalid)?;
    let canonical = serde_json::to_vec(&marker)
        .map_err(|error| snapshot_invalid(format!("Failed to encode active marker: {error}")))?;
    if canonical != bytes {
        return Err(snapshot_invalid(
            "The active restore marker is not canonically encoded.",
        ));
    }
    Ok(Some((marker, bytes.len() as u64, digest(&bytes))))
}

async fn validate_operation_directory(directory: &Path) -> UseResult<PathBuf> {
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| snapshot_io("read restore operation directory", error))?;
    let mut operation = None;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| snapshot_io("read restore operation entry", error))?
    {
        if operation.is_some() || entry.file_name() != "operation.json" {
            return Err(snapshot_invalid(
                "A restore operation directory contains temporary or unknown evidence.",
            ));
        }
        operation = Some(entry.path());
    }
    operation.ok_or_else(|| snapshot_invalid("A restore history directory has no operation."))
}

async fn read_optional_bounded_file(
    path: &Path,
    maximum: u64,
    label: &str,
) -> UseResult<Option<Vec<u8>>> {
    match fs::symlink_metadata(path).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(snapshot_io(&format!("inspect {label}"), error)),
        Ok(_) => read_bounded_file(path, maximum, label).await.map(Some),
    }
}

async fn read_bounded_file(path: &Path, maximum: u64, label: &str) -> UseResult<Vec<u8>> {
    let before = fs::symlink_metadata(path)
        .await
        .map_err(|error| snapshot_io(&format!("inspect {label}"), error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&before)
        || !before.is_file()
        || before.len() == 0
        || before.len() > maximum
    {
        return Err(snapshot_invalid(format!(
            "The {label} is not a bounded owned regular file."
        )));
    }
    let before_modified = before.modified().ok();
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ);
    }
    let mut file = options
        .open(path)
        .await
        .map_err(|error| snapshot_io(&format!("open {label}"), error))?;
    let capacity = usize::try_from(before.len())
        .map_err(|_| snapshot_invalid(format!("The {label} length is invalid.")))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .await
        .map_err(|error| snapshot_io(&format!("read {label}"), error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| snapshot_io(&format!("reinspect opened {label}"), error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| snapshot_io(&format!("reinspect {label}"), error))?;
    if bytes.len() as u64 != before.len()
        || a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != before.len()
        || after.len() != before.len()
        || a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || before_modified.is_some_and(|modified| after.modified().ok() != Some(modified))
    {
        return Err(snapshot_invalid(format!(
            "The {label} changed while it was read."
        )));
    }
    Ok(bytes)
}

async fn optional_owned_directory(path: &Path, label: &str) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(snapshot_invalid(format!(
            "The {label} is not an owned directory."
        ))),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(snapshot_io(&format!("inspect {label}"), error)),
    }
}

async fn validate_owned_directory(path: &Path, label: &str) -> UseResult<()> {
    if optional_owned_directory(path, label).await? {
        Ok(())
    } else {
        Err(snapshot_invalid(format!("The {label} is missing.")))
    }
}

async fn reject_existing_path(path: &Path, label: &str) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(snapshot_invalid(format!(
            "The {label} must be recovered before snapshot."
        ))),
        Err(error) => Err(snapshot_io(&format!("inspect {label}"), error)),
    }
}

fn valid_segment(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn wrap_invalid(error: UseError) -> UseError {
    snapshot_invalid(format!(
        "Restore history validation failed: {}",
        error.message
    ))
}

fn snapshot_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.state_restore_history_snapshot_invalid", message)
}

fn snapshot_io(action: &str, error: io::Error) -> UseError {
    snapshot_invalid(format!("Failed to {action}: {error}"))
}
