use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use sha2::Digest;
use tokio::fs;

use super::{read_owned_file, restore_io, valid_segment, OPERATION_FILE};
use crate::control_store::payload_owner::restore_coordinator::restore::restore_invalid;
use crate::control_store::payload_owner::restore_coordinator::ControlRestoreCoordinatorEntry;
use crate::state_restore::{
    inspect_terminal_state_restore_history_record, StateRestoreHistoryRetentionKey,
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES,
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ExactRestoreHistoryRecord {
    pub(super) evidence: ControlRestoreCoordinatorEntry,
    pub(super) retention: StateRestoreHistoryRetentionKey,
}

pub(super) async fn inspect_exact_tree(
    root: &Path,
    installation: &InstallationId,
) -> UseResult<Vec<ExactRestoreHistoryRecord>> {
    super::validate_directory(root).await?;
    let mut reader = fs::read_dir(root)
        .await
        .map_err(|error| restore_io("read Restore Coordinator record tree", error))?;
    let mut directories = Vec::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator record entry", error))?
    {
        if directories.len() >= STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES as usize {
            return Err(restore_invalid(
                "A Restore Coordinator record tree exceeds the native retention bound.",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Restore Coordinator record names must be valid UTF-8.")
        })?;
        if !valid_segment(&name) {
            return Err(restore_invalid(
                "A Restore Coordinator record directory has an invalid identity.",
            ));
        }
        let path = entry.path();
        super::validate_directory(&path).await?;
        directories.push((name, path));
    }
    directories.sort_by(|left, right| left.0.cmp(&right.0));

    let mut records = Vec::with_capacity(directories.len());
    for (segment, directory) in directories {
        records.push(inspect_record_directory(&directory, &segment, installation).await?);
    }
    Ok(records)
}

pub(super) async fn inspect_record_directory(
    directory: &Path,
    segment: &str,
    installation: &InstallationId,
) -> UseResult<ExactRestoreHistoryRecord> {
    super::validate_directory(directory).await?;
    let mut reader = fs::read_dir(directory)
        .await
        .map_err(|error| restore_io("read Restore Coordinator record directory", error))?;
    let mut operation = None;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator record file", error))?
    {
        if operation.is_some() || entry.file_name() != OPERATION_FILE {
            return Err(restore_invalid(
                "A Restore Coordinator record directory contains unknown evidence.",
            ));
        }
        operation = Some(entry.path());
    }
    let operation = operation.ok_or_else(|| {
        restore_invalid("A Restore Coordinator record directory has no operation evidence.")
    })?;
    let bytes = read_owned_file(
        &operation,
        STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
        "Restore Coordinator operation",
    )
    .await?;
    let plan_digest = format!("sha256:{segment}");
    let retention =
        inspect_terminal_state_restore_history_record(&plan_digest, &bytes, installation).map_err(
            |error| {
                restore_invalid(format!(
                    "Restore Coordinator native record validation failed: {}",
                    error.message
                ))
            },
        )?;
    Ok(ExactRestoreHistoryRecord {
        evidence: ControlRestoreCoordinatorEntry {
            plan_digest,
            length: bytes.len() as u64,
            sha256: format!("sha256:{:x}", sha2::Sha256::digest(&bytes)),
        },
        retention,
    })
}

pub(super) fn record_directory(root: &Path, plan_digest: &str) -> UseResult<PathBuf> {
    Ok(root.join(super::segment(plan_digest)?))
}

pub(super) fn operation_path(root: &Path, plan_digest: &str) -> UseResult<PathBuf> {
    Ok(record_directory(root, plan_digest)?.join(OPERATION_FILE))
}
