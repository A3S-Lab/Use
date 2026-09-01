use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::records::{self, ExactRestoreHistoryRecord};
use super::{
    candidate_path, ensure_owned_directory, optional_owned_directory, optional_regular_file_length,
    publish_noclobber, read_owned_file, restore_io, segment, sync_directory, validate_directory,
    OPERATION_FILE, OPERATION_PARTIAL_FILE,
};
use crate::control_store::payload_owner::restore_coordinator::restore::restore_invalid;
use crate::control_store::payload_owner::restore_coordinator::{
    archive, ControlRestoreCoordinatorEntry, ControlRestoreCoordinatorSnapshot,
    ControlRestoreCoordinatorState,
};
use crate::state_restore::{
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES,
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
};

#[derive(Debug)]
pub(in crate::control_store::payload_owner::restore_coordinator::restore) struct CanonicalRestoreHistory
{
    records: Vec<ExactRestoreHistoryRecord>,
}

impl CanonicalRestoreHistory {
    pub(super) fn absent() -> Self {
        Self {
            records: Vec::new(),
        }
    }

    pub(super) fn target(
        &self,
        active_plan_digest: &str,
        reserve_active_slot: bool,
    ) -> UseResult<(Vec<ControlRestoreCoordinatorEntry>, Option<String>)> {
        if self
            .records
            .iter()
            .any(|record| record.evidence.plan_digest == active_plan_digest)
        {
            return Err(restore_invalid(
                "The source restore history collides with the active restore identity.",
            ));
        }
        let mut target = self
            .records
            .iter()
            .map(|record| record.evidence.clone())
            .collect::<Vec<_>>();
        let pruned = if reserve_active_slot
            && self.records.len() == STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES as usize
        {
            let oldest = self
                .records
                .iter()
                .min_by(|left, right| left.retention.cmp(&right.retention))
                .ok_or_else(|| {
                    restore_invalid("No terminal Restore Coordinator record can be pruned.")
                })?;
            let index = target
                .binary_search_by(|entry| entry.plan_digest.cmp(&oldest.evidence.plan_digest))
                .map_err(|_| {
                    restore_invalid("The native oldest restore record is not in its inventory.")
                })?;
            target.remove(index);
            Some(oldest.evidence.plan_digest.clone())
        } else {
            None
        };
        Ok((target, pruned))
    }

    pub(super) fn record(&self, plan_digest: &str) -> Option<&ExactRestoreHistoryRecord> {
        self.records
            .binary_search_by(|record| record.evidence.plan_digest.as_str().cmp(plan_digest))
            .ok()
            .map(|index| &self.records[index])
    }
}

pub(super) async fn prepare(
    archive_path: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<CanonicalRestoreHistory> {
    let candidate = candidate_path(staging_directory);
    if !optional_owned_directory(&candidate).await? {
        ensure_owned_directory(staging_directory, &candidate).await?;
    }
    validate_build_tree(&candidate, snapshot).await?;
    let (archive_bytes, archive_sha256) = archive_evidence(snapshot)?;
    let mut reader = archive::RestoreCoordinatorArchiveReader::open(
        archive_path,
        archive_bytes,
        archive_sha256,
        &snapshot.manifest.entries,
        &snapshot.manifest.binding.installation,
    )
    .await
    .map_err(wrap_archive_error)?;
    for entry in &snapshot.manifest.entries {
        let bytes = reader
            .next()
            .await
            .map_err(wrap_archive_error)?
            .ok_or_else(|| {
                restore_invalid("The Restore Coordinator archive ended before its manifest.")
            })?;
        write_candidate_record(&candidate, entry, &bytes).await?;
    }
    if reader.next().await.map_err(wrap_archive_error)?.is_some() {
        return Err(restore_invalid(
            "The Restore Coordinator archive exceeds its manifest.",
        ));
    }
    reader.finish().await.map_err(wrap_archive_error)?;
    inspect(staging_directory, snapshot).await
}

pub(super) async fn inspect(
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<CanonicalRestoreHistory> {
    let candidate = candidate_path(staging_directory);
    if !optional_owned_directory(&candidate).await? {
        return Err(restore_invalid(
            "The Restore Coordinator candidate is missing.",
        ));
    }
    let records =
        records::inspect_exact_tree(&candidate, &snapshot.manifest.binding.installation).await?;
    let observed = records
        .iter()
        .map(|record| record.evidence.clone())
        .collect::<Vec<_>>();
    if observed != snapshot.manifest.entries {
        return Err(restore_invalid(
            "The Restore Coordinator candidate differs from its exact snapshot.",
        ));
    }
    Ok(CanonicalRestoreHistory { records })
}

async fn validate_build_tree(
    candidate: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<()> {
    validate_directory(candidate).await?;
    let expected = snapshot
        .manifest
        .entries
        .iter()
        .map(|entry| segment(&entry.plan_digest).map(str::to_owned))
        .collect::<UseResult<BTreeSet<_>>>()?;
    let mut reader = fs::read_dir(candidate)
        .await
        .map_err(|error| restore_io("read Restore Coordinator candidate", error))?;
    let mut count = 0_usize;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator candidate entry", error))?
    {
        count += 1;
        if count > expected.len() {
            return Err(restore_invalid(
                "The Restore Coordinator candidate contains too many records.",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Restore Coordinator candidate names must be valid UTF-8.")
        })?;
        if !expected.contains(&name) {
            return Err(restore_invalid(
                "The Restore Coordinator candidate contains an unknown record.",
            ));
        }
        validate_candidate_record_state(&entry.path()).await?;
    }
    Ok(())
}

async fn validate_candidate_record_state(directory: &Path) -> UseResult<()> {
    validate_directory(directory).await?;
    let mut reader = fs::read_dir(directory)
        .await
        .map_err(|error| restore_io("read staged Restore Coordinator record", error))?;
    let mut names = BTreeSet::new();
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read staged Restore Coordinator evidence", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Staged Restore Coordinator evidence names must be valid UTF-8.")
        })?;
        if !matches!(name.as_str(), OPERATION_FILE | OPERATION_PARTIAL_FILE) || !names.insert(name)
        {
            return Err(restore_invalid(
                "A staged Restore Coordinator record contains unknown evidence.",
            ));
        }
        optional_regular_file_length(&entry.path()).await?;
    }
    if names.len() > 1 {
        return Err(restore_invalid(
            "A staged Restore Coordinator record has ambiguous publication state.",
        ));
    }
    Ok(())
}

async fn write_candidate_record(
    candidate: &Path,
    entry: &ControlRestoreCoordinatorEntry,
    bytes: &[u8],
) -> UseResult<()> {
    if bytes.len() as u64 != entry.length || digest(bytes) != entry.sha256 {
        return Err(restore_invalid(
            "A Restore Coordinator archive record differs from its manifest.",
        ));
    }
    let directory = records::record_directory(candidate, &entry.plan_digest)?;
    if !optional_owned_directory(&directory).await? {
        ensure_owned_directory(candidate, &directory).await?;
    }
    validate_candidate_record_state(&directory).await?;
    let target = directory.join(OPERATION_FILE);
    let partial = directory.join(OPERATION_PARTIAL_FILE);
    let target_length = optional_regular_file_length(&target).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if target_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "A Restore Coordinator candidate record has ambiguous bytes.",
        ));
    }
    if let Some(length) = target_length {
        if length != entry.length
            || read_owned_file(
                &target,
                STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
                "Restore Coordinator candidate record",
            )
            .await?
                != bytes
        {
            return Err(restore_invalid(
                "An existing Restore Coordinator candidate record was modified.",
            ));
        }
        return Ok(());
    }
    if let Some(length) = partial_length {
        if length == entry.length
            && read_owned_file(
                &partial,
                STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
                "partial Restore Coordinator candidate record",
            )
            .await?
                == bytes
        {
            publish_noclobber(
                partial,
                target,
                "publish Restore Coordinator candidate record",
            )
            .await?;
            sync_directory(&directory).await?;
            return Ok(());
        }
        if length >= entry.length {
            return Err(restore_invalid(
                "A partial Restore Coordinator candidate has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial).await.map_err(|error| {
            restore_io("remove incomplete Restore Coordinator candidate", error)
        })?;
        sync_directory(&directory).await?;
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create Restore Coordinator candidate partial", error))?;
    output
        .write_all(bytes)
        .await
        .map_err(|error| restore_io("write Restore Coordinator candidate", error))?;
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush Restore Coordinator candidate", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Restore Coordinator candidate", error))?;
    drop(output);
    sync_directory(&directory).await?;
    if read_owned_file(
        &partial,
        STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
        "partial Restore Coordinator candidate record",
    )
    .await?
        != bytes
    {
        return Err(restore_invalid(
            "A Restore Coordinator candidate changed before publication.",
        ));
    }
    publish_noclobber(
        partial,
        target,
        "publish Restore Coordinator candidate record",
    )
    .await?;
    sync_directory(&directory).await
}

fn archive_evidence(snapshot: &ControlRestoreCoordinatorSnapshot) -> UseResult<(u64, &str)> {
    match &snapshot.manifest.payload {
        ControlRestoreCoordinatorState::Archive {
            archive_bytes,
            archive_sha256,
        } => Ok((*archive_bytes, archive_sha256)),
        ControlRestoreCoordinatorState::Absent => Err(restore_invalid(
            "An absent Restore Coordinator snapshot has no archive evidence.",
        )),
    }
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn wrap_archive_error(error: a3s_use_core::UseError) -> a3s_use_core::UseError {
    restore_invalid(format!(
        "Restore Coordinator archive verification failed: {}",
        error.message
    ))
}
