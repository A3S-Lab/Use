use std::path::Path;

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{rehydration_state_invalid, ArtifactRehydrationRecord};
use crate::artifact_store::quarantine::canonical_json;
use crate::package::{io_error, sync_parent_directory};

pub(in crate::artifact_store) const REHYDRATION_PREPARED_RECORD: &str = "rehydration-plan.json";
pub(in crate::artifact_store) const REHYDRATION_PREPARED_TEMPORARY: &str = ".rehydration-plan.tmp";
pub(in crate::artifact_store) const REHYDRATION_RECORD: &str = "rehydration.json";
pub(in crate::artifact_store) const REHYDRATION_TEMPORARY: &str = ".rehydration.tmp";
const MAX_REHYDRATION_RECORD_BYTES: u64 = 16 * 1024;

#[derive(Debug)]
pub(in crate::artifact_store) enum ContainerRehydrationState {
    None,
    InterruptedPreparation,
    Prepared(ArtifactRehydrationRecord),
    InterruptedCompletion(ArtifactRehydrationRecord),
    Rehydrated(ArtifactRehydrationRecord),
}

pub(in crate::artifact_store) async fn inspect_container_state(
    container: &Path,
) -> UseResult<ContainerRehydrationState> {
    let prepared_path = container.join(REHYDRATION_PREPARED_RECORD);
    let prepared_temporary_path = container.join(REHYDRATION_PREPARED_TEMPORARY);
    let completed_path = container.join(REHYDRATION_RECORD);
    let completed_temporary_path = container.join(REHYDRATION_TEMPORARY);
    let prepared = optional_record(&prepared_path).await?;
    let prepared_temporary = optional_temporary(&prepared_temporary_path).await?;
    let completed = optional_record(&completed_path).await?;
    let completed_temporary = optional_temporary(&completed_temporary_path).await?;

    if prepared.is_some() && prepared_temporary {
        return Err(rehydration_state_invalid(
            "An Artifact Store container has both prepared and temporary rehydration plans.",
        ));
    }
    if completed.is_some() && completed_temporary {
        return Err(rehydration_state_invalid(
            "An Artifact Store container has both completed and temporary rehydration records.",
        ));
    }
    if prepared_temporary && (completed.is_some() || completed_temporary) {
        return Err(rehydration_state_invalid(
            "An interrupted rehydration preparation has unexpected completion state.",
        ));
    }
    match (prepared, prepared_temporary, completed, completed_temporary) {
        (None, false, None, false) => Ok(ContainerRehydrationState::None),
        (None, true, None, false) => Ok(ContainerRehydrationState::InterruptedPreparation),
        (Some(prepared), false, None, false) => Ok(ContainerRehydrationState::Prepared(prepared)),
        (Some(prepared), false, None, true) => {
            Ok(ContainerRehydrationState::InterruptedCompletion(prepared))
        }
        (Some(prepared), false, Some(completed), false) if prepared == completed => {
            Ok(ContainerRehydrationState::Rehydrated(completed))
        }
        (Some(_), false, Some(_), false) => Err(rehydration_state_invalid(
            "The completed Artifact Store rehydration record differs from its prepared plan.",
        )),
        _ => Err(rehydration_state_invalid(
            "An Artifact Store container has an invalid rehydration state transition.",
        )),
    }
}

pub(in crate::artifact_store) fn validate_rehydration_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    allow_empty: bool,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || (!allow_empty && metadata.len() == 0)
        || metadata.len() > MAX_REHYDRATION_RECORD_BYTES
    {
        return Err(rehydration_state_invalid(format!(
            "Artifact Store rehydration state '{}' is not a bounded owned regular file.",
            path.display()
        )));
    }
    Ok(())
}

pub(super) async fn write_prepared_record(
    container: &Path,
    record: &ArtifactRehydrationRecord,
    recover_interrupted: bool,
) -> UseResult<()> {
    write_record(
        container,
        REHYDRATION_PREPARED_RECORD,
        REHYDRATION_PREPARED_TEMPORARY,
        record,
        recover_interrupted,
        "prepared Artifact Store rehydration plan",
    )
    .await
}

pub(super) async fn write_completed_record(
    container: &Path,
    record: &ArtifactRehydrationRecord,
    recover_interrupted: bool,
) -> UseResult<()> {
    write_record(
        container,
        REHYDRATION_RECORD,
        REHYDRATION_TEMPORARY,
        record,
        recover_interrupted,
        "completed Artifact Store rehydration record",
    )
    .await
}

async fn optional_record(path: &Path) -> UseResult<Option<ArtifactRehydrationRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error(
                "inspect Artifact Store rehydration record",
                path,
                error,
            ))
        }
    };
    validate_rehydration_metadata(path, &metadata, false)?;
    load_record(path).await.map(Some)
}

async fn optional_temporary(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            validate_rehydration_metadata(path, &metadata, true)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(
            "inspect Artifact Store rehydration temporary",
            path,
            error,
        )),
    }
}

async fn load_record(path: &Path) -> UseResult<ArtifactRehydrationRecord> {
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
        .map_err(|error| io_error("open Artifact Store rehydration record", path, error))?;
    let metadata = file.metadata().await.map_err(|error| {
        io_error(
            "inspect opened Artifact Store rehydration record",
            path,
            error,
        )
    })?;
    validate_rehydration_metadata(path, &metadata, false)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_REHYDRATION_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read Artifact Store rehydration record", path, error))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(rehydration_state_invalid(
            "The Artifact Store rehydration record changed while it was read.",
        ));
    }
    let record: ArtifactRehydrationRecord = serde_json::from_slice(&bytes).map_err(|error| {
        rehydration_state_invalid(format!(
            "The Artifact Store rehydration record is invalid JSON: {error}"
        ))
    })?;
    record.validate()?;
    if canonical_json(&record)? != bytes {
        return Err(rehydration_state_invalid(
            "The Artifact Store rehydration record is not canonical JSON.",
        ));
    }
    Ok(record)
}

async fn write_record(
    container: &Path,
    record_name: &str,
    temporary_name: &str,
    record: &ArtifactRehydrationRecord,
    recover_interrupted: bool,
    label: &str,
) -> UseResult<()> {
    let path = container.join(record_name);
    let temporary = container.join(temporary_name);
    let bytes = canonical_json(record)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_REHYDRATION_RECORD_BYTES {
        return Err(rehydration_state_invalid(
            "The generated Artifact Store rehydration record exceeds its storage bound.",
        ));
    }
    let mut options = fs::OpenOptions::new();
    options.write(true);
    if recover_interrupted {
        options.truncate(true);
    } else {
        options.create_new(true);
    }
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
        .open(&temporary)
        .await
        .map_err(|error| io_error(&format!("open {label}"), &temporary, error))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| io_error(&format!("inspect opened {label}"), &temporary, error))?;
    validate_rehydration_metadata(&temporary, &metadata, true)?;
    file.write_all(&bytes)
        .await
        .map_err(|error| io_error(&format!("write {label}"), &temporary, error))?;
    file.sync_all()
        .await
        .map_err(|error| io_error(&format!("sync {label}"), &temporary, error))?;
    drop(file);

    let path_for_worker = path.clone();
    let published = tokio::task::spawn_blocking(move || {
        crate::atomic_file::persist_temporary_noclobber_retain_blocking(temporary, &path_for_worker)
    })
    .await
    .map_err(|error| {
        rehydration_state_invalid(format!(
            "Artifact Store rehydration publication worker did not complete: {error}"
        ))
    })?;
    if let Err(error) = published {
        return Err(io_error(&format!("publish {label}"), &path, error));
    }
    sync_parent_directory(container, label).await
}
