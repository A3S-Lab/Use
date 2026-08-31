use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::restore::{restore_staging_invalid, restore_staging_io};

pub(super) const ATTEMPT_DIRECTORY: &str = ".control-installation-restore";
pub(super) const CONTROL_DIRECTORY: &str = "control";
pub(super) const HOST_PROJECTION_DIRECTORY: &str = "host-projection";
pub(super) const KNOWLEDGE_DIRECTORY: &str = "knowledge";
pub(super) const OBSERVATIONS_DIRECTORY: &str = "observations";
pub(super) const RESTORE_COORDINATOR_DIRECTORY: &str = "restore-coordinator";

const ATTEMPT_FILE: &str = "attempt.json";
const ATTEMPT_PARTIAL_FILE: &str = "attempt.json.partial";
const MAX_ATTEMPT_BYTES: u64 = 128 * 1024;

pub(super) async fn prepare_attempt(
    state_root: &Path,
    expected_evidence: &[u8],
) -> UseResult<PathBuf> {
    validate_state_root_inventory(state_root).await?;
    let attempt = state_root.join(ATTEMPT_DIRECTORY);
    let created = match fs::create_dir(&attempt).await {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
        Err(error) => {
            return Err(restore_staging_io(
                "create restore attempt directory",
                error,
            ))
        }
    };
    validate_directory(&attempt).await?;
    if created {
        sync_directory(state_root).await?;
    }
    validate_attempt_entries(&attempt).await?;
    publish_attempt_evidence(&attempt, expected_evidence).await?;
    validate_state_root_inventory(state_root).await?;
    validate_attempt_entries(&attempt).await?;
    Ok(attempt)
}

pub(super) async fn validate_complete_attempt(
    state_root: &Path,
    attempt: &Path,
    expected_evidence: &[u8],
) -> UseResult<()> {
    validate_state_root_inventory(state_root).await?;
    validate_attempt_entries(attempt).await?;
    let evidence = attempt.join(ATTEMPT_FILE);
    let Some(evidence_length) = optional_regular_file_length(&evidence).await? else {
        return Err(restore_staging_invalid(
            "The complete restore attempt has no durable exact descriptor.",
        ));
    };
    if read_exact_owned(&evidence, evidence_length).await? != expected_evidence
        || optional_regular_file_length(&attempt.join(ATTEMPT_PARTIAL_FILE))
            .await?
            .is_some()
    {
        return Err(restore_staging_invalid(
            "The complete restore attempt has no durable exact descriptor.",
        ));
    }
    for name in component_names() {
        validate_directory(&attempt.join(name)).await?;
    }
    Ok(())
}

pub(super) fn component_directory(attempt: &Path, name: &'static str) -> PathBuf {
    attempt.join(name)
}

async fn validate_state_root_inventory(state_root: &Path) -> UseResult<()> {
    validate_directory(state_root).await?;
    let mut entries = fs::read_dir(state_root)
        .await
        .map_err(|error| restore_staging_io("read target state root", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_staging_io("read target state entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_staging_invalid("The target state root contains a non-UTF-8 entry.")
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_staging_io("inspect target state entry", error))?;
        let valid = if crate::installation_state_layout::excluded_root_lock(&name) {
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_file()
        } else if name == ATTEMPT_DIRECTORY {
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir()
        } else {
            false
        };
        if !valid {
            return Err(restore_staging_invalid(
                "A complete restore requires a clean target containing only operational locks and its exact staging attempt.",
            ));
        }
    }
    Ok(())
}

async fn validate_attempt_entries(attempt: &Path) -> UseResult<()> {
    validate_directory(attempt).await?;
    let component_names = component_names().into_iter().collect::<BTreeSet<_>>();
    let mut entries = fs::read_dir(attempt)
        .await
        .map_err(|error| restore_staging_io("read complete restore attempt", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_staging_io("read complete restore attempt entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_staging_invalid("The complete restore attempt contains a non-UTF-8 entry.")
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_staging_io("inspect complete restore attempt entry", error))?;
        let valid = if name == ATTEMPT_FILE || name == ATTEMPT_PARTIAL_FILE {
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_file()
        } else if component_names.contains(name.as_str()) {
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir()
        } else {
            false
        };
        if !valid {
            return Err(restore_staging_invalid(
                "The complete restore attempt contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

async fn publish_attempt_evidence(attempt: &Path, expected: &[u8]) -> UseResult<()> {
    if expected.is_empty() || expected.len() as u64 > MAX_ATTEMPT_BYTES {
        return Err(restore_staging_invalid(
            "The complete restore attempt descriptor exceeds its byte bound.",
        ));
    }
    let target = attempt.join(ATTEMPT_FILE);
    let partial = attempt.join(ATTEMPT_PARTIAL_FILE);
    let target_length = optional_regular_file_length(&target).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if let Some(length) = target_length {
        if partial_length.is_some() || read_exact_owned(&target, length).await? != expected {
            return Err(restore_staging_invalid(
                "The complete restore attempt descriptor was changed or rebound.",
            ));
        }
        return Ok(());
    }

    if attempt_contains_component(attempt).await? {
        return Err(restore_staging_invalid(
            "A complete restore attempt contains candidates without a durable descriptor.",
        ));
    }
    if let Some(length) = partial_length {
        let bytes = read_exact_owned(&partial, length).await?;
        if bytes == expected {
            publish_noclobber(partial, target).await?;
            sync_directory(attempt).await?;
            return Ok(());
        }
        if length >= expected.len() as u64 {
            return Err(restore_staging_invalid(
                "The partial complete restore descriptor has unexpected bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_staging_io("remove incomplete restore descriptor", error))?;
    }

    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_staging_io("create partial restore descriptor", error))?;
    file.write_all(expected)
        .await
        .map_err(|error| restore_staging_io("write partial restore descriptor", error))?;
    file.flush()
        .await
        .map_err(|error| restore_staging_io("flush partial restore descriptor", error))?;
    file.sync_all()
        .await
        .map_err(|error| restore_staging_io("sync partial restore descriptor", error))?;
    drop(file);
    if read_exact_owned(&partial, expected.len() as u64).await? != expected {
        return Err(restore_staging_invalid(
            "The partial complete restore descriptor changed while it was written.",
        ));
    }
    publish_noclobber(partial, target).await?;
    sync_directory(attempt).await
}

async fn attempt_contains_component(attempt: &Path) -> UseResult<bool> {
    for name in component_names() {
        match fs::symlink_metadata(attempt.join(name)).await {
            Ok(_) => return Ok(true),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(restore_staging_io(
                    "inspect restore attempt component",
                    error,
                ))
            }
        }
    }
    Ok(false)
}

fn component_names() -> [&'static str; 5] {
    [
        CONTROL_DIRECTORY,
        HOST_PROJECTION_DIRECTORY,
        KNOWLEDGE_DIRECTORY,
        OBSERVATIONS_DIRECTORY,
        RESTORE_COORDINATOR_DIRECTORY,
    ]
}

async fn publish_noclobber(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| {
        restore_staging_invalid(format!(
            "The restore descriptor publication worker did not complete: {error}"
        ))
    })?
    .map_err(|error| {
        restore_staging_invalid(format!(
            "Failed to publish restore descriptor '{}': {error}",
            error_target.display()
        ))
    })
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
            "A complete restore evidence path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_staging_io(
            "inspect complete restore evidence",
            error,
        )),
    }
}

async fn read_exact_owned(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    if expected_length > MAX_ATTEMPT_BYTES {
        return Err(restore_staging_invalid(
            "A complete restore evidence file exceeds its byte bound.",
        ));
    }
    let mut file = fs::File::open(path)
        .await
        .map_err(|error| restore_staging_io("open complete restore evidence", error))?;
    let capacity = usize::try_from(expected_length)
        .map_err(|_| restore_staging_invalid("A complete restore evidence length overflowed."))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_staging_io("read complete restore evidence", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(restore_staging_invalid(
            "A complete restore evidence file changed while it was read.",
        ));
    }
    Ok(bytes)
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_staging_io("inspect complete restore directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_staging_invalid(
            "A complete restore directory is not an owned directory.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_staging_io("open complete restore directory for sync", error))?
        .sync_all()
        .await
        .map_err(|error| restore_staging_io("sync complete restore directory", error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
