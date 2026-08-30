use std::path::Path;

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    canonical_json, deletion, garbage_collection_in_progress, garbage_collection_state_invalid,
    ArtifactGarbageCollectionRecord,
};
use crate::package::{io_error, remove_file_with_windows_retry, sync_parent_directory};

pub(in crate::artifact_store) const GARBAGE_COLLECTION_PREPARED_RECORD: &str =
    "garbage-collection-plan.json";
pub(in crate::artifact_store) const GARBAGE_COLLECTION_PREPARED_TEMPORARY: &str =
    ".garbage-collection-plan.tmp";
pub(in crate::artifact_store) const GARBAGE_COLLECTION_COMPLETED_RECORD: &str =
    "garbage-collection.json";
pub(in crate::artifact_store) const GARBAGE_COLLECTION_COMPLETED_TEMPORARY: &str =
    ".garbage-collection.tmp";
const MAX_GARBAGE_COLLECTION_RECORD_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug)]
pub(in crate::artifact_store) enum GarbageCollectionState {
    None,
    InterruptedPreparation {
        predecessor: Option<ArtifactGarbageCollectionRecord>,
    },
    Prepared {
        record: ArtifactGarbageCollectionRecord,
        predecessor: Option<ArtifactGarbageCollectionRecord>,
    },
    InterruptedCompletion {
        record: ArtifactGarbageCollectionRecord,
    },
    Completed {
        record: ArtifactGarbageCollectionRecord,
        prepared_record_present: bool,
    },
}

pub(in crate::artifact_store) async fn ensure_reference_admission_allowed(
    root: &Path,
) -> UseResult<()> {
    let prepared = metadata_present(&root.join(GARBAGE_COLLECTION_PREPARED_RECORD), false).await?;
    let prepared_temporary =
        metadata_present(&root.join(GARBAGE_COLLECTION_PREPARED_TEMPORARY), true).await?;
    let completed_temporary =
        metadata_present(&root.join(GARBAGE_COLLECTION_COMPLETED_TEMPORARY), true).await?;
    // A completed-only record is replay evidence, not a mutation fence. Keep the
    // admission hot path bounded to metadata validation; maintenance performs the
    // full record and tombstone validation before another collection.
    metadata_present(&root.join(GARBAGE_COLLECTION_COMPLETED_RECORD), true).await?;

    if !prepared && !prepared_temporary && !completed_temporary {
        return Ok(());
    }
    match inspect_state(root).await? {
        GarbageCollectionState::Completed { record, .. } => {
            deletion::require_no_tombstones_at_root(root, &record).await
        }
        _ => Err(garbage_collection_in_progress()),
    }
}

pub(in crate::artifact_store) async fn inspect_state(
    root: &Path,
) -> UseResult<GarbageCollectionState> {
    let prepared_path = root.join(GARBAGE_COLLECTION_PREPARED_RECORD);
    let prepared_temporary_path = root.join(GARBAGE_COLLECTION_PREPARED_TEMPORARY);
    let completed_path = root.join(GARBAGE_COLLECTION_COMPLETED_RECORD);
    let completed_temporary_path = root.join(GARBAGE_COLLECTION_COMPLETED_TEMPORARY);
    let prepared = optional_record(&prepared_path).await?;
    let prepared_temporary = optional_temporary(&prepared_temporary_path).await?;
    let completed = optional_record(&completed_path).await?;
    let completed_temporary = optional_temporary(&completed_temporary_path).await?;

    if prepared.is_some() && prepared_temporary {
        return Err(garbage_collection_state_invalid(
            "The Artifact Store has both prepared and temporary garbage-collection plans.",
        ));
    }
    if completed.is_some() && completed_temporary {
        return Err(garbage_collection_state_invalid(
            "The Artifact Store has both completed and temporary garbage-collection records.",
        ));
    }
    if prepared_temporary && (prepared.is_some() || completed_temporary) {
        return Err(garbage_collection_state_invalid(
            "An interrupted garbage-collection preparation has unexpected durable state.",
        ));
    }
    if completed_temporary && prepared.is_none() {
        return Err(garbage_collection_state_invalid(
            "An interrupted garbage-collection completion has no prepared plan.",
        ));
    }

    match (prepared, prepared_temporary, completed, completed_temporary) {
        (None, false, None, false) => Ok(GarbageCollectionState::None),
        (None, true, predecessor, false) => {
            Ok(GarbageCollectionState::InterruptedPreparation { predecessor })
        }
        (Some(record), false, None, false) => Ok(GarbageCollectionState::Prepared {
            record,
            predecessor: None,
        }),
        (Some(record), false, None, true) => {
            Ok(GarbageCollectionState::InterruptedCompletion { record })
        }
        (Some(record), false, Some(completed), false) if record == completed => {
            Ok(GarbageCollectionState::Completed {
                record,
                prepared_record_present: true,
            })
        }
        (Some(record), false, Some(predecessor), false)
            if record.plan.predecessor_plan_digest.as_deref()
                == Some(predecessor.plan_digest.as_str()) =>
        {
            Ok(GarbageCollectionState::Prepared {
                record,
                predecessor: Some(predecessor),
            })
        }
        (None, false, Some(record), false) => Ok(GarbageCollectionState::Completed {
            record,
            prepared_record_present: false,
        }),
        _ => Err(garbage_collection_state_invalid(
            "The Artifact Store has an invalid garbage-collection state transition.",
        )),
    }
}

pub(in crate::artifact_store) fn validate_state_metadata(
    path: &Path,
    metadata: &std::fs::Metadata,
    allow_empty: bool,
) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata)
        || !metadata.is_file()
        || (!allow_empty && metadata.len() == 0)
        || metadata.len() > MAX_GARBAGE_COLLECTION_RECORD_BYTES
    {
        return Err(garbage_collection_state_invalid(format!(
            "Artifact Store garbage-collection state '{}' is not a bounded owned regular file.",
            path.display()
        )));
    }
    Ok(())
}

pub(super) async fn write_prepared_record(
    root: &Path,
    record: &ArtifactGarbageCollectionRecord,
    recover_interrupted: bool,
) -> UseResult<()> {
    write_record(
        root,
        GARBAGE_COLLECTION_PREPARED_RECORD,
        GARBAGE_COLLECTION_PREPARED_TEMPORARY,
        record,
        recover_interrupted,
        "prepared Artifact Store garbage-collection plan",
    )
    .await
}

pub(super) async fn write_completed_record(
    root: &Path,
    record: &ArtifactGarbageCollectionRecord,
    recover_interrupted: bool,
) -> UseResult<()> {
    write_record(
        root,
        GARBAGE_COLLECTION_COMPLETED_RECORD,
        GARBAGE_COLLECTION_COMPLETED_TEMPORARY,
        record,
        recover_interrupted,
        "completed Artifact Store garbage-collection record",
    )
    .await
}

pub(super) async fn remove_prepared_record(root: &Path) -> UseResult<()> {
    remove_record(
        root,
        GARBAGE_COLLECTION_PREPARED_RECORD,
        "remove completed Artifact Store garbage-collection plan",
    )
    .await
}

pub(super) async fn remove_completed_record(root: &Path) -> UseResult<()> {
    remove_record(
        root,
        GARBAGE_COLLECTION_COMPLETED_RECORD,
        "retire predecessor Artifact Store garbage-collection record",
    )
    .await
}

async fn optional_record(path: &Path) -> UseResult<Option<ArtifactGarbageCollectionRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(io_error(
                "inspect Artifact Store garbage-collection record",
                path,
                error,
            ))
        }
    };
    validate_state_metadata(path, &metadata, false)?;
    load_record(path).await.map(Some)
}

async fn optional_temporary(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            validate_state_metadata(path, &metadata, true)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(
            "inspect Artifact Store garbage-collection temporary",
            path,
            error,
        )),
    }
}

async fn metadata_present(path: &Path, allow_empty: bool) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            validate_state_metadata(path, &metadata, allow_empty)?;
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(io_error(
            "inspect Artifact Store garbage-collection state",
            path,
            error,
        )),
    }
}

async fn load_record(path: &Path) -> UseResult<ArtifactGarbageCollectionRecord> {
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
        .map_err(|error| io_error("open Artifact Store garbage-collection record", path, error))?;
    let metadata = file.metadata().await.map_err(|error| {
        io_error(
            "inspect opened Artifact Store garbage-collection record",
            path,
            error,
        )
    })?;
    validate_state_metadata(path, &metadata, false)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    (&mut file)
        .take(MAX_GARBAGE_COLLECTION_RECORD_BYTES.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| io_error("read Artifact Store garbage-collection record", path, error))?;
    if bytes.len() as u64 != metadata.len() {
        return Err(garbage_collection_state_invalid(
            "The Artifact Store garbage-collection record changed while it was read.",
        ));
    }
    let record: ArtifactGarbageCollectionRecord =
        serde_json::from_slice(&bytes).map_err(|error| {
            garbage_collection_state_invalid(format!(
                "The Artifact Store garbage-collection record is invalid JSON: {error}"
            ))
        })?;
    record.validate()?;
    if canonical_json(&record)? != bytes {
        return Err(garbage_collection_state_invalid(
            "The Artifact Store garbage-collection record is not canonical JSON.",
        ));
    }
    Ok(record)
}

async fn write_record(
    root: &Path,
    record_name: &str,
    temporary_name: &str,
    record: &ArtifactGarbageCollectionRecord,
    recover_interrupted: bool,
    label: &str,
) -> UseResult<()> {
    record.validate()?;
    let path = root.join(record_name);
    let temporary = root.join(temporary_name);
    let bytes = canonical_json(record)?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_GARBAGE_COLLECTION_RECORD_BYTES {
        return Err(garbage_collection_state_invalid(
            "The generated Artifact Store garbage-collection record exceeds its storage bound.",
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
    validate_state_metadata(&temporary, &metadata, true)?;
    file.write_all(&bytes)
        .await
        .map_err(|error| io_error(&format!("write {label}"), &temporary, error))?;
    file.sync_all()
        .await
        .map_err(|error| io_error(&format!("sync {label}"), &temporary, error))?;
    drop(file);

    let path_for_worker = path.clone();
    tokio::task::spawn_blocking(move || {
        crate::atomic_file::persist_temporary_noclobber_retain_blocking(temporary, &path_for_worker)
    })
    .await
    .map_err(|error| {
        garbage_collection_state_invalid(format!(
            "Artifact Store garbage-collection publication worker did not complete: {error}"
        ))
    })?
    .map_err(|error| io_error(&format!("publish {label}"), &path, error))?;
    sync_parent_directory(root, label).await
}

async fn remove_record(root: &Path, name: &str, action: &'static str) -> UseResult<()> {
    let path = root.join(name);
    let metadata = fs::symlink_metadata(&path).await.map_err(|error| {
        io_error(
            "inspect Artifact Store garbage-collection record",
            &path,
            error,
        )
    })?;
    validate_state_metadata(&path, &metadata, false)?;
    remove_file_with_windows_retry(path, action).await?;
    sync_parent_directory(root, "Artifact Store garbage-collection state").await
}
