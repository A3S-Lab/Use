use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::ControlPayloadSnapshotBinding;
use super::control_restore_evidence::{ControlCandidateEvidence, MAX_CONTROL_CANDIDATE_BYTES};
use super::restore::{restore_staging_invalid, restore_staging_io};
use super::ControlPayloadOwnerRegistry;
use crate::control_store::executor::ControlStoreExecutor;

const CANDIDATE_FILE: &str = "control.sqlite3";
const PARTIAL_FILE: &str = "control.sqlite3.partial";
const EVIDENCE_FILE: &str = "candidate.json";
const EVIDENCE_PARTIAL_FILE: &str = "candidate.json.partial";
const MAX_EVIDENCE_BYTES: u64 = 128 * 1024;

#[derive(Debug)]
pub(super) struct StagedControlStoreRestore {
    candidate: PathBuf,
}

impl StagedControlStoreRestore {
    pub(super) fn candidate_path(&self) -> &Path {
        &self.candidate
    }
}

pub(super) async fn stage(
    registry: &ControlPayloadOwnerRegistry,
    snapshot_descriptor_digest: &str,
    binding: &ControlPayloadSnapshotBinding,
    control_export: &[u8],
    state_root: &Path,
    staging_directory: &Path,
    maintenance: &StateMaintenanceGuard,
) -> UseResult<StagedControlStoreRestore> {
    if !maintenance.is_exclusive_for(state_root) {
        return Err(restore_staging_invalid(
            "Control candidate staging requires the exact target's exclusive maintenance guard.",
        ));
    }
    let verified = binding
        .verify_control_export(registry, control_export)
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The complete restore Control export is invalid: {}",
                error.message
            ))
        })?;
    ensure_owned_directory(state_root, staging_directory).await?;
    validate_entries(staging_directory).await?;
    let physical_directory = fs::canonicalize(staging_directory)
        .await
        .map_err(|error| restore_staging_io("resolve Control candidate directory", error))?;
    validate_directory(&physical_directory).await?;
    let candidate = staging_directory.join(CANDIDATE_FILE);
    let physical_candidate = physical_directory.join(CANDIDATE_FILE);
    let partial = physical_directory.join(PARTIAL_FILE);
    let candidate_exists = optional_regular_file(&candidate).await?;
    let partial_exists = optional_regular_file(&staging_directory.join(PARTIAL_FILE)).await?;
    let evidence_exists = optional_regular_file(&staging_directory.join(EVIDENCE_FILE)).await?;
    let evidence_partial_exists =
        optional_regular_file(&staging_directory.join(EVIDENCE_PARTIAL_FILE)).await?;

    if candidate_exists {
        if partial_exists {
            return Err(restore_staging_invalid(
                "The complete restore Control candidate has conflicting partial state.",
            ));
        }
    } else {
        if evidence_exists || evidence_partial_exists {
            return Err(restore_staging_invalid(
                "Control candidate evidence exists without its database.",
            ));
        }
        if partial_exists || any_sidecar(staging_directory).await? {
            remove_partial_family(staging_directory).await?;
        }
        let executor = ControlStoreExecutor::new().map_err(|error| {
            restore_staging_invalid(format!(
                "Failed to start Control candidate worker: {}",
                error.message
            ))
        })?;
        let restore = executor
            .restore(
                partial.clone(),
                binding.installation.clone(),
                verified.export,
            )
            .await;
        if let Err(error) = restore {
            remove_partial_family(staging_directory).await?;
            return Err(restore_staging_invalid(format!(
                "Failed to build Control restore candidate: {}",
                error.message
            )));
        }
        verify_exact_candidate(&executor, &partial, binding, control_export).await?;
        fs::OpenOptions::new()
            .write(true)
            .open(&partial)
            .await
            .map_err(|error| restore_staging_io("open Control candidate for sync", error))?
            .sync_all()
            .await
            .map_err(|error| restore_staging_io("sync Control candidate", error))?;
        publish_noclobber(partial, physical_candidate.clone()).await?;
        sync_directory(&physical_directory).await?;
    }

    require_no_sidecars(staging_directory).await?;
    let executor = ControlStoreExecutor::new().map_err(|error| {
        restore_staging_invalid(format!(
            "Failed to start Control verification worker: {}",
            error.message
        ))
    })?;
    let (database_bytes, database_sha256) =
        verify_exact_candidate(&executor, &physical_candidate, binding, control_export).await?;
    let evidence = ControlCandidateEvidence::new(
        registry,
        snapshot_descriptor_digest,
        binding,
        database_bytes,
        database_sha256,
    )?;
    publish_evidence(
        staging_directory,
        &evidence.canonical_bytes(MAX_EVIDENCE_BYTES)?,
    )
    .await?;
    validate_entries(staging_directory).await?;
    require_no_sidecars(staging_directory).await?;
    Ok(StagedControlStoreRestore { candidate })
}

async fn verify_exact_candidate(
    executor: &ControlStoreExecutor,
    candidate: &Path,
    binding: &ControlPayloadSnapshotBinding,
    expected_export: &[u8],
) -> UseResult<(u64, String)> {
    let exported = executor
        .export(candidate.to_path_buf(), binding.installation.clone())
        .await
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The staged Control database failed verification: {}",
                error.message
            ))
        })?
        .into_bytes();
    if exported != expected_export {
        return Err(restore_staging_invalid(
            "The staged Control database does not reproduce the exact bound export.",
        ));
    }
    cleanup_quiescent_sidecars(candidate).await?;
    hash_owned_file(candidate).await
}

async fn publish_evidence(directory: &Path, expected: &[u8]) -> UseResult<()> {
    let target = directory.join(EVIDENCE_FILE);
    let partial = directory.join(EVIDENCE_PARTIAL_FILE);
    if let Some(length) = optional_regular_file_length(&target).await? {
        if optional_regular_file(&partial).await?
            || read_exact_owned(&target, length, MAX_EVIDENCE_BYTES).await? != expected
        {
            return Err(restore_staging_invalid(
                "The Control candidate evidence was changed or rebound.",
            ));
        }
        return Ok(());
    }
    if let Some(length) = optional_regular_file_length(&partial).await? {
        let bytes = read_exact_owned(&partial, length, MAX_EVIDENCE_BYTES).await?;
        if bytes == expected {
            publish_noclobber(partial, target).await?;
            sync_directory(directory).await?;
            return Ok(());
        }
        if length >= expected.len() as u64 {
            return Err(restore_staging_invalid(
                "The partial Control candidate evidence has unexpected bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_staging_io("remove partial Control evidence", error))?;
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_staging_io("create partial Control evidence", error))?;
    file.write_all(expected)
        .await
        .map_err(|error| restore_staging_io("write partial Control evidence", error))?;
    file.flush()
        .await
        .map_err(|error| restore_staging_io("flush partial Control evidence", error))?;
    file.sync_all()
        .await
        .map_err(|error| restore_staging_io("sync partial Control evidence", error))?;
    drop(file);
    if read_exact_owned(&partial, expected.len() as u64, MAX_EVIDENCE_BYTES).await? != expected {
        return Err(restore_staging_invalid(
            "The partial Control candidate evidence changed while it was written.",
        ));
    }
    publish_noclobber(partial, target).await?;
    sync_directory(directory).await
}

async fn ensure_owned_directory(state_root: &Path, target: &Path) -> UseResult<()> {
    if target == state_root || !target.starts_with(state_root) {
        return Err(restore_staging_invalid(
            "The Control candidate directory escapes its target state root.",
        ));
    }
    validate_directory(state_root).await?;
    let relative = target
        .strip_prefix(state_root)
        .map_err(|_| restore_staging_invalid("The Control candidate path is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_staging_invalid(
            "The Control candidate path is not normalized.",
        ));
    }
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {
                if let Some(parent) = current.parent() {
                    sync_directory(parent).await?;
                }
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(restore_staging_io(
                    "create Control candidate directory",
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_entries(directory: &Path) -> UseResult<()> {
    validate_directory(directory).await?;
    let allowed = BTreeSet::from([
        CANDIDATE_FILE,
        PARTIAL_FILE,
        EVIDENCE_FILE,
        EVIDENCE_PARTIAL_FILE,
        "control.sqlite3-wal",
        "control.sqlite3-shm",
        "control.sqlite3-journal",
        "control.sqlite3.partial-wal",
        "control.sqlite3.partial-shm",
        "control.sqlite3.partial-journal",
    ]);
    let mut entries = fs::read_dir(directory)
        .await
        .map_err(|error| restore_staging_io("read Control candidate directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_staging_io("read Control candidate entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_staging_invalid("The Control candidate contains a non-UTF-8 entry.")
        })?;
        if !allowed.contains(name.as_str()) || !optional_regular_file(&entry.path()).await? {
            return Err(restore_staging_invalid(
                "The Control candidate directory contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

async fn remove_partial_family(directory: &Path) -> UseResult<()> {
    for name in [
        PARTIAL_FILE,
        "control.sqlite3.partial-wal",
        "control.sqlite3.partial-shm",
        "control.sqlite3.partial-journal",
    ] {
        let path = directory.join(name);
        if optional_regular_file(&path).await? {
            fs::remove_file(&path)
                .await
                .map_err(|error| restore_staging_io("remove partial Control candidate", error))?;
        }
    }
    sync_directory(directory).await
}

async fn require_no_sidecars(directory: &Path) -> UseResult<()> {
    if any_sidecar(directory).await? {
        return Err(restore_staging_invalid(
            "The staged Control database retained an operational SQLite sidecar.",
        ));
    }
    Ok(())
}

async fn cleanup_quiescent_sidecars(candidate: &Path) -> UseResult<()> {
    let file_name = candidate
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            restore_staging_invalid("The staged Control database has no UTF-8 file name.")
        })?;
    let parent = candidate.parent().ok_or_else(|| {
        restore_staging_invalid("The staged Control database has no owned parent.")
    })?;
    for suffix in ["-wal", "-journal"] {
        let path = parent.join(format!("{file_name}{suffix}"));
        if optional_regular_file_length(&path)
            .await?
            .is_some_and(|length| length != 0)
        {
            return Err(restore_staging_invalid(
                "The staged Control database retained uncheckpointed SQLite bytes.",
            ));
        }
    }
    for suffix in ["-wal", "-shm", "-journal"] {
        remove_quiescent_sidecar(&parent.join(format!("{file_name}{suffix}"))).await?;
    }
    sync_directory(parent).await
}

#[cfg(windows)]
async fn remove_quiescent_sidecar(path: &Path) -> UseResult<()> {
    let mut attempts = 0_u8;
    loop {
        match fs::remove_file(path).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if attempts < 80
                    && (error.kind() == io::ErrorKind::PermissionDenied
                        || matches!(error.raw_os_error(), Some(32 | 33))) =>
            {
                attempts = attempts.saturating_add(1);
                tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            }
            Err(error) => {
                return Err(restore_staging_io(
                    "remove quiescent Control SQLite sidecar",
                    error,
                ))
            }
        }
    }
}

#[cfg(not(windows))]
async fn remove_quiescent_sidecar(path: &Path) -> UseResult<()> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_staging_io(
            "remove quiescent Control SQLite sidecar",
            error,
        )),
    }
}

async fn any_sidecar(directory: &Path) -> UseResult<bool> {
    for name in [
        "control.sqlite3-wal",
        "control.sqlite3-shm",
        "control.sqlite3-journal",
        "control.sqlite3.partial-wal",
        "control.sqlite3.partial-shm",
        "control.sqlite3.partial-journal",
    ] {
        if optional_regular_file(&directory.join(name)).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn hash_owned_file(path: &Path) -> UseResult<(u64, String)> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_staging_io("inspect staged Control database", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_CONTROL_CANDIDATE_BYTES
    {
        return Err(restore_staging_invalid(
            "The staged Control database is not a bounded owned regular file.",
        ));
    }
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| restore_staging_io("open staged Control database", error))?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| restore_staging_io("read staged Control database", error))?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or_else(|| restore_staging_invalid("Control candidate byte count overflowed."))?;
        if total > MAX_CONTROL_CANDIDATE_BYTES {
            return Err(restore_staging_invalid(
                "The staged Control database exceeds its byte bound.",
            ));
        }
        digest.update(&buffer[..read]);
    }
    if total != metadata.len() {
        return Err(restore_staging_invalid(
            "The staged Control database changed while it was read.",
        ));
    }
    Ok((total, format!("sha256:{:x}", digest.finalize())))
}

async fn read_exact_owned(path: &Path, expected: u64, maximum: u64) -> UseResult<Vec<u8>> {
    if expected > maximum {
        return Err(restore_staging_invalid(
            "A staged Control evidence file exceeds its byte bound.",
        ));
    }
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| restore_staging_io("open staged Control evidence", error))?;
    let capacity = usize::try_from(expected)
        .map_err(|_| restore_staging_invalid("Control evidence length overflowed."))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(expected.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_staging_io("read staged Control evidence", error))?;
    if bytes.len() as u64 != expected {
        return Err(restore_staging_invalid(
            "A staged Control evidence file changed while it was read.",
        ));
    }
    Ok(bytes)
}

async fn optional_regular_file(path: &Path) -> UseResult<bool> {
    Ok(optional_regular_file_length(path).await?.is_some())
}

async fn optional_regular_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_staging_invalid(
            "A staged Control path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_staging_io("inspect staged Control path", error)),
    }
}

async fn publish_noclobber(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| {
        restore_staging_invalid(format!(
            "The Control candidate publication worker did not complete: {error}"
        ))
    })?
    .map_err(|error| {
        restore_staging_invalid(format!(
            "Failed to publish staged Control file '{}': {error}",
            error_target.display()
        ))
    })
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_staging_io("inspect Control candidate directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_staging_invalid(
            "A Control candidate directory is not an owned directory.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_staging_io("open Control candidate directory for sync", error))?
        .sync_all()
        .await
        .map_err(|error| restore_staging_io("sync Control candidate directory", error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
