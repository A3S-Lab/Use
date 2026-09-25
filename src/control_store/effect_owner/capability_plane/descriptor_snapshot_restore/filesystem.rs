//! Filesystem helpers for descriptor-snapshot clean-target restore.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use serde::Serialize;
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::{
    canonical_json, ensure_directory_exists, ensure_owned_directory_chain, file_identity,
    metadata_is_link, path_for_digest, scan_records, sync_directory, validate_existing_directory,
    validate_regular_file, write_new_record, ControlCapabilityDescriptorSnapshotStore,
    SNAPSHOT_STAGING,
};
use super::{
    digest, reject_foreign_staging, restore_invalid, restore_io, restore_result,
    restore_target_not_empty, staging_directory, valid_digest, validate_candidate_layout,
    validate_restore_staging_layout, ACTIVATION_FILE, ACTIVATION_PARTIAL_FILE, ACTIVATION_SCHEMA,
    CANDIDATE_DIRECTORY, MAX_ACTIVATION_BYTES, MAX_RESTORE_BYTES, PreparedSnapshot,
    ControlCapabilityDescriptorSnapshotRestoreEntry,
    ControlCapabilityDescriptorSnapshotRestorePlan,
};


pub(crate) enum LiveSnapshotRoot {
    Absent,
    Owned(Vec<ControlCapabilityDescriptorSnapshotRestoreEntry>),
}

pub(crate) async fn inspect_live(
    store: &ControlCapabilityDescriptorSnapshotStore,
) -> UseResult<LiveSnapshotRoot> {
    if !validate_existing_directory(&store.root).await? {
        return Ok(LiveSnapshotRoot::Absent);
    }
    let records = scan_records(&store.root, &store.installation).await?;
    super::super::retention::ensure_no_pending_journal(&store.root).await?;
    if staged_records_present(&store.root).await? {
        return Err(restore_invalid(
            "The live descriptor snapshot owner contains residual staging evidence.",
        ));
    }
    let mut entries = records
        .iter()
        .map(ControlCapabilityDescriptorSnapshotRestoreEntry::from_snapshot)
        .collect::<UseResult<Vec<_>>>()?;
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    Ok(LiveSnapshotRoot::Owned(entries))
}

pub(crate) async fn staged_records_present(root: &Path) -> UseResult<bool> {
    let staging = root.join(SNAPSHOT_STAGING);
    if !validate_existing_directory(&staging).await? {
        return Ok(false);
    }
    let mut entries = tokio_fs::read_dir(&staging)
        .await
        .map_err(|error| restore_io("read descriptor snapshot staging", error))?;
    Ok(entries
        .next_entry()
        .await
        .map_err(|error| restore_io("inspect descriptor snapshot staging", error))?
        .is_some())
}

pub(crate) async fn prepare_staging(
    store: &ControlCapabilityDescriptorSnapshotStore,
    staging: &Path,
    records: &[PreparedSnapshot],
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    ensure_owned_directory_chain(&store.state_root, staging).await?;
    validate_restore_staging_layout(staging).await?;
    if recover_activation_marker(staging, plan, plan_digest).await? {
        let candidate = staging.join(CANDIDATE_DIRECTORY);
        if !validate_existing_directory(&candidate).await? {
            return Err(restore_invalid(
                "The descriptor snapshot restore candidate disappeared after activation began.",
            ));
        }
        return validate_candidate(store, &candidate, &plan.records).await;
    }
    let candidate = staging.join(CANDIDATE_DIRECTORY);
    if validate_existing_directory(&candidate).await? {
        if validate_candidate(store, &candidate, &plan.records)
            .await
            .is_ok()
        {
            return Ok(());
        }
        remove_candidate(staging, &candidate).await?;
    }
    ensure_owned_directory_chain(staging, &candidate).await?;
    for snapshot in records {
        let target = path_for_digest(&candidate, &snapshot.entry.digest)?;
        write_new_record(&candidate, &target, &snapshot.bytes).await?;
    }
    validate_candidate(store, &candidate, &plan.records).await
}

pub(crate) async fn validate_candidate(
    store: &ControlCapabilityDescriptorSnapshotStore,
    candidate: &Path,
    expected: &[ControlCapabilityDescriptorSnapshotRestoreEntry],
) -> UseResult<()> {
    validate_candidate_layout(candidate).await?;
    let records = scan_records(candidate, &store.installation).await?;
    let mut entries = records
        .iter()
        .map(ControlCapabilityDescriptorSnapshotRestoreEntry::from_snapshot)
        .collect::<UseResult<Vec<_>>>()?;
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    if entries != expected {
        return Err(restore_invalid(
            "The staged descriptor snapshot inventory differs from its plan.",
        ));
    }
    Ok(())
}

pub(crate) async fn remove_candidate(staging: &Path, candidate: &Path) -> UseResult<()> {
    if candidate.parent() != Some(staging)
        || candidate.file_name().and_then(|name| name.to_str()) != Some(CANDIDATE_DIRECTORY)
    {
        return Err(restore_invalid(
            "The descriptor snapshot restore candidate escapes its staging directory.",
        ));
    }
    let metadata = tokio_fs::symlink_metadata(candidate)
        .await
        .map_err(|error| restore_io("inspect descriptor snapshot restore candidate", error))?;
    if metadata_is_link(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "The descriptor snapshot restore candidate is not an owned directory.",
        ));
    }
    let candidate = candidate.to_path_buf();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::remove_dir_all_with_windows_retry_blocking(&candidate)
    })
    .await
    .map_err(|error| {
        restore_invalid(format!(
            "Descriptor snapshot candidate cleanup failed: {error}"
        ))
    })?
    .map_err(|error| restore_io("remove descriptor snapshot restore candidate", error))?;
    sync_directory(staging).await
}

pub(crate) async fn recover_activation_marker(
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<bool> {
    let expected = activation_bytes(plan, plan_digest)?;
    let marker = staging.join(ACTIVATION_FILE);
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker_length = optional_file_length(&marker).await?;
    let partial_length = optional_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The descriptor snapshot restore activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_exact_owned(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The descriptor snapshot restore activation marker differs from its plan.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        tokio_fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete descriptor restore marker", error))?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_exact_owned(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A complete descriptor restore marker partial has unexpected bytes.",
        ));
    }
    publish_noclobber(
        partial,
        marker,
        "publish descriptor restore activation marker",
    )
    .await?;
    sync_directory(staging).await?;
    Ok(true)
}

pub(crate) async fn create_activation_marker(
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    if recover_activation_marker(staging, plan, plan_digest).await? {
        return Ok(());
    }
    let bytes = activation_bytes(plan, plan_digest)?;
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker = staging.join(ACTIVATION_FILE);
    let mut options = tokio_fs::OpenOptions::new();
    options.create_new(true).write(true);
    super::super::configure_no_follow(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&partial)
        .await
        .map_err(|error| restore_io("create descriptor restore activation marker", error))?;
    file.write_all(&bytes)
        .await
        .map_err(|error| restore_io("write descriptor restore activation marker", error))?;
    file.flush()
        .await
        .map_err(|error| restore_io("flush descriptor restore activation marker", error))?;
    file.sync_all()
        .await
        .map_err(|error| restore_io("sync descriptor restore activation marker", error))?;
    drop(file);
    if read_exact_owned(&partial, bytes.len() as u64).await? != bytes {
        return Err(restore_invalid(
            "The descriptor restore activation marker changed before publication.",
        ));
    }
    publish_noclobber(
        partial,
        marker,
        "publish descriptor restore activation marker",
    )
    .await?;
    sync_directory(staging).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Activation<'a> {
    schema: &'static str,
    installation: &'a InstallationId,
    plan_digest: &'a str,
    inventory_digest: &'a str,
    record_count: u64,
    byte_count: u64,
}

pub(crate) fn activation_bytes(
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<Vec<u8>> {
    plan.validate()?;
    if !valid_digest(plan_digest) || plan.descriptor_digest()? != plan_digest {
        return Err(restore_invalid(
            "The descriptor restore activation digest differs from its plan.",
        ));
    }
    let bytes = canonical_json(
        &Activation {
            schema: ACTIVATION_SCHEMA,
            installation: &plan.installation,
            plan_digest,
            inventory_digest: &plan.inventory_digest,
            record_count: plan.record_count,
            byte_count: plan.byte_count,
        },
        "descriptor snapshot restore activation",
    )?;
    if bytes.is_empty() || bytes.len() > MAX_ACTIVATION_BYTES {
        return Err(restore_invalid(
            "The descriptor restore activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

pub(crate) async fn publish_candidate(candidate: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_retain_blocking(candidate, &target)
    })
    .await
    .map_err(|error| {
        restore_invalid(format!(
            "Descriptor restore publication worker failed: {error}"
        ))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            restore_io(
                &format!(
                    "publish descriptor restore target '{}'",
                    error_target.display()
                ),
                error,
            )
        }
    })?;
    sync_directory(
        error_target.parent().ok_or_else(|| {
            restore_invalid("The descriptor restore target has no parent directory.")
        })?,
    )
    .await
}

pub(crate) async fn retire_completed_staging(
    store: &ControlCapabilityDescriptorSnapshotStore,
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    match tokio_fs::symlink_metadata(staging).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_io(
            "inspect completed descriptor restore staging",
            error,
        )),
        Ok(metadata) => {
            if metadata_is_link(&metadata) || !metadata.is_dir() {
                return Err(restore_invalid(
                    "The completed descriptor restore staging path is not an owned directory.",
                ));
            }
            validate_restore_staging_layout(staging).await?;
            let activation_started = recover_activation_marker(staging, plan, plan_digest).await?;
            let candidate = staging.join(CANDIDATE_DIRECTORY);
            let candidate_exists = validate_existing_directory(&candidate).await?;
            if candidate_exists {
                validate_candidate(store, &candidate, &plan.records).await?;
                remove_candidate(staging, &candidate).await?;
            }
            if activation_started {
                retire_staging(staging, plan, plan_digest).await
            } else if candidate_exists {
                retire_unmarked_staging(staging).await
            } else {
                Err(restore_invalid(
                    "The completed descriptor restore has ambiguous staged evidence.",
                ))
            }
        }
    }
}

pub(crate) async fn retire_staging(
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    validate_restore_staging_layout(staging).await?;
    if !recover_activation_marker(staging, plan, plan_digest).await?
        || validate_existing_directory(&staging.join(CANDIDATE_DIRECTORY)).await?
    {
        return Err(restore_invalid(
            "The descriptor restore staging directory cannot be retired before activation.",
        ));
    }
    let marker = staging.join(ACTIVATION_FILE);
    tokio_fs::remove_file(&marker)
        .await
        .map_err(|error| restore_io("retire descriptor restore activation marker", error))?;
    sync_directory(staging).await?;
    let mut entries = tokio_fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read retired descriptor restore staging", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("finish descriptor restore staging", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The descriptor restore staging directory contains residual evidence.",
        ));
    }
    tokio_fs::remove_dir(staging)
        .await
        .map_err(|error| restore_io("retire descriptor restore staging", error))?;
    sync_directory(staging.parent().ok_or_else(|| {
        restore_invalid("The descriptor restore staging directory has no parent.")
    })?)
    .await
}

pub(crate) async fn retire_unmarked_staging(staging: &Path) -> UseResult<()> {
    validate_restore_staging_layout(staging).await?;
    let mut entries = tokio_fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read unmarked descriptor restore staging", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("finish unmarked descriptor restore staging", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The unmarked descriptor restore staging directory contains residual evidence.",
        ));
    }
    tokio_fs::remove_dir(staging)
        .await
        .map_err(|error| restore_io("retire unmarked descriptor restore staging", error))?;
    sync_directory(staging.parent().ok_or_else(|| {
        restore_invalid("The descriptor restore staging directory has no parent.")
    })?)
    .await
}

pub(crate) async fn reject_unexpected_staging(staging: &Path) -> UseResult<()> {
    match tokio_fs::symlink_metadata(staging).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_io(
            "inspect empty descriptor restore staging",
            error,
        )),
        Ok(_) => Err(restore_invalid(
            "An empty descriptor restore plan has unexpected staged evidence.",
        )),
    }
}

pub(crate) async fn optional_file_length(path: &Path) -> UseResult<Option<u64>> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link(&metadata) && metadata.is_file() => {
            if metadata.len() == 0 || metadata.len() > MAX_ACTIVATION_BYTES as u64 {
                return Err(restore_invalid(
                    "A descriptor restore activation marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "A descriptor restore activation marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io(
            "inspect descriptor restore activation marker",
            error,
        )),
    }
}

pub(crate) async fn read_exact_owned(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    validate_regular_file(path).await?;
    let metadata = tokio_fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect descriptor restore activation marker", error))?;
    if metadata.len() != expected_length || expected_length > MAX_ACTIVATION_BYTES as u64 {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed before it was read.",
        ));
    }
    let before = file_identity(&metadata);
    let mut options = tokio_fs::OpenOptions::new();
    options.read(true);
    super::super::configure_no_follow(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| restore_io("open descriptor restore activation marker", error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| restore_io("inspect opened descriptor restore marker", error))?;
    if !opened.is_file() || opened.len() != expected_length || file_identity(&opened) != before {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed while opened.",
        ));
    }
    let mut bytes = Vec::with_capacity(expected_length as usize);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_io("read descriptor restore activation marker", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed while read.",
        ));
    }
    let after = tokio_fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("reinspect descriptor restore activation marker", error))?;
    if metadata_is_link(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || after.len() != expected_length
    {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed after it was read.",
        ));
    }
    Ok(bytes)
}

pub(crate) async fn publish_noclobber(source: PathBuf, target: PathBuf, action: &str) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join {action}: {error}")))?
    .map_err(|error| restore_io(&format!("{action} '{}'", error_target.display()), error))
}
