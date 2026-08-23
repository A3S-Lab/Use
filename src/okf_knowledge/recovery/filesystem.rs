use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::journal::{
    RestoreFileEvidence, RestoreOperationPaths, RestorePriorFiles, MAX_RESTORE_FILE_BYTES,
};
use crate::okf_knowledge::{OkfKnowledgeBackupManifest, ScopeDatabaseGuard};

const COPY_BUFFER_BYTES: usize = 1024 * 1024;

pub(super) async fn capture_prior_files(
    guard: &ScopeDatabaseGuard,
) -> UseResult<RestorePriorFiles> {
    let database = optional_evidence(guard.path()).await?;
    let wal_path = sidecar_path(guard.path(), "-wal");
    let shm_path = sidecar_path(guard.path(), "-shm");
    let wal = optional_evidence(&wal_path).await?;
    let shm = optional_evidence(&shm_path).await?;
    if database.is_none() && (wal.is_some() || shm.is_some()) {
        return Err(filesystem_invalid(
            "Knowledge database sidecars exist without an owned main database.",
        ));
    }
    Ok(RestorePriorFiles { database, wal, shm })
}

pub(super) async fn ensure_candidate(
    paths: &RestoreOperationPaths,
    verified_database: Option<&Path>,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<()> {
    let expected = backup_evidence(manifest);
    match optional_evidence(&paths.candidate).await? {
        Some(current) if current == expected => return Ok(()),
        Some(_) => {
            return Err(filesystem_invalid(
                "The staged Knowledge restore candidate differs from the reviewed backup.",
            ));
        }
        None => {}
    }

    let partial = paths.directory.join("candidate.sqlite3.partial");
    if let Some(current) = optional_evidence(&partial).await? {
        if current == expected {
            activate_file(&partial, &paths.candidate).await?;
            return Ok(());
        }
        if current.bytes >= expected.bytes {
            return Err(filesystem_invalid(
                "The partial Knowledge restore candidate has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial).await.map_err(|error| {
            filesystem_io(
                "remove incomplete Knowledge restore candidate",
                &partial,
                error,
            )
        })?;
        sync_directory(&paths.directory).await?;
    }

    let verified_database = verified_database.ok_or_else(|| {
        filesystem_invalid(
            "The durable restore candidate is missing and the reviewed backup is unavailable for restaging.",
        )
    })?;
    let source = required_evidence(verified_database).await?;
    if source != expected {
        return Err(filesystem_invalid(
            "The verified Knowledge backup snapshot changed before restore staging.",
        ));
    }
    copy_exact(verified_database, &partial, &expected).await?;
    activate_file(&partial, &paths.candidate).await
}

pub(super) async fn ensure_prior_moved(
    guard: &ScopeDatabaseGuard,
    paths: &RestoreOperationPaths,
    prior: &RestorePriorFiles,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<()> {
    require_candidate(paths, manifest).await?;
    let live_wal = sidecar_path(guard.path(), "-wal");
    let live_shm = sidecar_path(guard.path(), "-shm");
    move_expected(
        &live_wal,
        &paths.prior_wal,
        prior.wal.as_ref(),
        "prior-wal-moved",
    )
    .await?;
    move_expected(
        &live_shm,
        &paths.prior_shm,
        prior.shm.as_ref(),
        "prior-shm-moved",
    )
    .await?;
    move_expected(
        guard.path(),
        &paths.prior_database,
        prior.database.as_ref(),
        "prior-database-moved",
    )
    .await?;
    validate_prior_moved(guard, paths, prior).await
}

pub(super) async fn ensure_published(
    guard: &ScopeDatabaseGuard,
    paths: &RestoreOperationPaths,
    prior: &RestorePriorFiles,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<()> {
    if validate_published(guard, paths, prior, manifest)
        .await
        .is_ok()
    {
        return Ok(());
    }
    validate_prior_moved(guard, paths, prior).await?;
    let expected = backup_evidence(manifest);
    let candidate = optional_evidence(&paths.candidate).await?;
    let live = optional_evidence(guard.path()).await?;
    match (candidate, live) {
        (Some(candidate), None) if candidate == expected => {
            activate_file(&paths.candidate, guard.path()).await?;
        }
        (None, Some(live)) if live == expected => {}
        _ => {
            return Err(filesystem_invalid(
                "The Knowledge restore candidate and live database do not match a recoverable publication state.",
            ));
        }
    }
    validate_published(guard, paths, prior, manifest).await
}

pub(super) async fn validate_published(
    guard: &ScopeDatabaseGuard,
    paths: &RestoreOperationPaths,
    prior: &RestorePriorFiles,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<()> {
    validate_prior_targets(paths, prior).await?;
    if optional_evidence(&paths.candidate).await?.is_some()
        || optional_evidence(&paths.directory.join("candidate.sqlite3.partial"))
            .await?
            .is_some()
        || optional_evidence(&sidecar_path(guard.path(), "-wal"))
            .await?
            .is_some()
        || optional_evidence(&sidecar_path(guard.path(), "-shm"))
            .await?
            .is_some()
        || required_evidence(guard.path()).await? != backup_evidence(manifest)
    {
        return Err(filesystem_invalid(
            "The published Knowledge restore filesystem evidence is incomplete or has drifted.",
        ));
    }
    Ok(())
}

async fn require_candidate(
    paths: &RestoreOperationPaths,
    manifest: &OkfKnowledgeBackupManifest,
) -> UseResult<()> {
    if optional_evidence(&paths.candidate).await? != Some(backup_evidence(manifest)) {
        return Err(filesystem_invalid(
            "The Knowledge restore candidate is missing or differs from the reviewed backup.",
        ));
    }
    Ok(())
}

async fn validate_prior_moved(
    guard: &ScopeDatabaseGuard,
    paths: &RestoreOperationPaths,
    prior: &RestorePriorFiles,
) -> UseResult<()> {
    if optional_evidence(guard.path()).await?.is_some()
        || optional_evidence(&sidecar_path(guard.path(), "-wal"))
            .await?
            .is_some()
        || optional_evidence(&sidecar_path(guard.path(), "-shm"))
            .await?
            .is_some()
    {
        return Err(filesystem_invalid(
            "The prior Knowledge database has not been completely moved into restore-owned retention.",
        ));
    }
    validate_prior_targets(paths, prior).await
}

async fn validate_prior_targets(
    paths: &RestoreOperationPaths,
    prior: &RestorePriorFiles,
) -> UseResult<()> {
    validate_optional_expected(&paths.prior_database, prior.database.as_ref()).await?;
    validate_optional_expected(&paths.prior_wal, prior.wal.as_ref()).await?;
    validate_optional_expected(&paths.prior_shm, prior.shm.as_ref()).await
}

async fn move_expected(
    source: &Path,
    destination: &Path,
    expected: Option<&RestoreFileEvidence>,
    checkpoint: &str,
) -> UseResult<()> {
    let source_evidence = optional_evidence(source).await?;
    let destination_evidence = optional_evidence(destination).await?;
    match (expected, source_evidence, destination_evidence) {
        (None, None, None) => Ok(()),
        (Some(expected), Some(source_evidence), None) if &source_evidence == expected => {
            activate_file(source, destination).await?;
            maybe_test_crash(checkpoint);
            Ok(())
        }
        (Some(expected), None, Some(destination)) if &destination == expected => Ok(()),
        _ => Err(filesystem_invalid(
            "A prior Knowledge database file is missing, duplicated, or differs from its durable restore evidence.",
        )),
    }
}

#[cfg(test)]
fn maybe_test_crash(checkpoint: &str) {
    if std::env::var(super::RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok(checkpoint) {
        std::process::exit(86);
    }
}

#[cfg(not(test))]
fn maybe_test_crash(_checkpoint: &str) {}

async fn validate_optional_expected(
    path: &Path,
    expected: Option<&RestoreFileEvidence>,
) -> UseResult<()> {
    let actual = optional_evidence(path).await?;
    if actual.as_ref() != expected {
        return Err(filesystem_invalid(
            "A restore-retained prior Knowledge file differs from its durable evidence.",
        ));
    }
    Ok(())
}

async fn copy_exact(
    source: &Path,
    destination: &Path,
    expected: &RestoreFileEvidence,
) -> UseResult<()> {
    let mut source_file = fs::File::open(source)
        .await
        .map_err(|error| filesystem_io("open verified Knowledge restore source", source, error))?;
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut destination_file = options.open(destination).await.map_err(|error| {
        filesystem_io(
            "create partial Knowledge restore candidate",
            destination,
            error,
        )
    })?;
    let mut remaining = expected.bytes;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64)).map_err(|_| {
            filesystem_invalid("The Knowledge restore copy length exceeds the platform range.")
        })?;
        let count = source_file
            .read(&mut buffer[..requested])
            .await
            .map_err(|error| filesystem_io("read Knowledge restore source", source, error))?;
        if count == 0 {
            return Err(filesystem_invalid(
                "The Knowledge restore source ended before its reviewed length.",
            ));
        }
        destination_file
            .write_all(&buffer[..count])
            .await
            .map_err(|error| {
                filesystem_io(
                    "write partial Knowledge restore candidate",
                    destination,
                    error,
                )
            })?;
        digest.update(&buffer[..count]);
        remaining -= u64::try_from(count).map_err(|_| {
            filesystem_invalid("The Knowledge restore copy count exceeds the platform range.")
        })?;
    }
    let mut extra = [0_u8; 1];
    if source_file
        .read(&mut extra)
        .await
        .map_err(|error| filesystem_io("recheck Knowledge restore source", source, error))?
        != 0
    {
        return Err(filesystem_invalid(
            "The Knowledge restore source grew beyond its reviewed length.",
        ));
    }
    destination_file.sync_all().await.map_err(|error| {
        filesystem_io(
            "sync partial Knowledge restore candidate",
            destination,
            error,
        )
    })?;
    drop(destination_file);
    let actual = RestoreFileEvidence {
        bytes: expected.bytes,
        sha256: format!("sha256:{:x}", digest.finalize()),
    };
    if &actual != expected {
        return Err(filesystem_invalid(
            "The staged Knowledge restore candidate digest differs from the reviewed backup.",
        ));
    }
    sync_parent(destination).await
}

async fn activate_file(source: &Path, destination: &Path) -> UseResult<()> {
    if optional_evidence(destination).await?.is_some() {
        return Err(filesystem_invalid(
            "A Knowledge restore activation target already exists.",
        ));
    }
    let source_path = source.to_path_buf();
    let destination_path = destination.to_path_buf();
    let error_destination = destination_path.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::rename_path_with_windows_retry_blocking(&source_path, &destination_path)
    })
    .await
    .map_err(|error| {
        filesystem_invalid(format!(
            "Knowledge restore activation worker did not complete: {error}"
        ))
    })?
    .map_err(|error| filesystem_io("activate Knowledge restore file", &error_destination, error))?;
    sync_parent(source).await?;
    let source_parent = source.parent();
    if destination.parent() != source_parent {
        sync_parent(destination).await?;
    }
    Ok(())
}

async fn required_evidence(path: &Path) -> UseResult<RestoreFileEvidence> {
    optional_evidence(path).await?.ok_or_else(|| {
        filesystem_invalid(format!(
            "The required Knowledge restore file '{}' is missing.",
            path.display()
        ))
    })
}

async fn optional_evidence(path: &Path) -> UseResult<Option<RestoreFileEvidence>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(filesystem_io("inspect Knowledge restore file", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_RESTORE_FILE_BYTES
    {
        return Err(filesystem_invalid(format!(
            "Knowledge restore file '{}' is not a bounded owned regular file.",
            path.display()
        )));
    }
    let bytes = metadata.len();
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| filesystem_io("open Knowledge restore file", path, error))?;
    let mut remaining = bytes;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64)).map_err(|_| {
            filesystem_invalid("The Knowledge restore hash length exceeds the platform range.")
        })?;
        let count = file
            .read(&mut buffer[..requested])
            .await
            .map_err(|error| filesystem_io("hash Knowledge restore file", path, error))?;
        if count == 0 {
            return Err(filesystem_invalid(
                "A Knowledge restore file changed while its evidence was read.",
            ));
        }
        digest.update(&buffer[..count]);
        remaining -= u64::try_from(count).map_err(|_| {
            filesystem_invalid("The Knowledge restore hash count exceeds the platform range.")
        })?;
    }
    let mut extra = [0_u8; 1];
    if file
        .read(&mut extra)
        .await
        .map_err(|error| filesystem_io("recheck Knowledge restore file", path, error))?
        != 0
    {
        return Err(filesystem_invalid(
            "A Knowledge restore file grew while its evidence was read.",
        ));
    }
    Ok(Some(RestoreFileEvidence {
        bytes,
        sha256: format!("sha256:{:x}", digest.finalize()),
    }))
}

fn backup_evidence(manifest: &OkfKnowledgeBackupManifest) -> RestoreFileEvidence {
    RestoreFileEvidence {
        bytes: manifest.database_bytes,
        sha256: manifest.database_sha256.clone(),
    }
}

fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(database.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

async fn sync_parent(path: &Path) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        filesystem_invalid("A Knowledge restore file path has no parent directory.")
    })?;
    sync_directory(parent).await
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| filesystem_io("open Knowledge restore directory", path, error))?
        .sync_all()
        .await
        .map_err(|error| filesystem_io("sync Knowledge restore directory", path, error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn filesystem_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_restore_filesystem_invalid", message)
}

fn filesystem_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.okf.knowledge_restore_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
