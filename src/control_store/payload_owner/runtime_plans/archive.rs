//! Archive encoding, verification, and staging for Runtime plan payload snapshots.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;
use sha2::{Digest, Sha256};
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::super::ControlPayloadOwnerLimits;
use crate::control_store::model::valid_sha256;
use crate::plugin_runtime::{
    RuntimeSurfacePlanStore, RuntimeSurfacePlanStoredRecord,
};

use super::{
    ARCHIVE_FILE, ARCHIVE_PARTIAL_FILE, ControlRuntimePlanPayloadEntry,
    ControlRuntimePlanPayloadSnapshot, ControlRuntimePlanPayloadState,
    MAX_ARCHIVE_RECORD_BYTES, CANDIDATE_DIRECTORY,
};
use super::filesystem::{
    open_owned_file, optional_owned_file, path_exists, publish_noclobber, sync_directory,
};
use super::helpers::{
    digest_bytes, restore_invalid, runtime_plan_error, runtime_plan_io, wrap_plan_error,
};


pub(super) struct CapturedRuntimePlans {
    pub(super) payload: ControlRuntimePlanPayloadState,
    pub(super) entries: Vec<ControlRuntimePlanPayloadEntry>,
    pub(super) archive_path: Option<PathBuf>,
}

pub(super) async fn snapshot_live(
    store: &RuntimeSurfacePlanStore,
    maintenance: &StateMaintenanceGuard,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<CapturedRuntimePlans> {
    let records = store
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_plan_error)?;
    if records.is_empty() {
        if path_exists(&destination).await? {
            return Err(runtime_plan_error(
                "An absent Runtime plan payload has an unexpected archive file.",
            ));
        }
        return Ok(CapturedRuntimePlans {
            payload: ControlRuntimePlanPayloadState::Absent,
            entries: Vec::new(),
            archive_path: None,
        });
    }
    let mut entries = Vec::with_capacity(records.len());
    let mut total = 0_u64;
    for record in &records {
        let key_digest = record.key.descriptor_digest().map_err(wrap_plan_error)?;
        let length = u64::try_from(record.bytes.len())
            .map_err(|_| runtime_plan_error("Runtime plan record length overflowed."))?;
        total = total.checked_add(length).ok_or_else(|| {
            runtime_plan_error("Runtime plan archive byte accounting overflowed.")
        })?;
        entries.push(ControlRuntimePlanPayloadEntry {
            key: record.key.clone(),
            key_digest,
            length,
            sha256: digest_bytes(&record.bytes),
        });
    }
    if entries.len() as u64 > limits.max_files || total > limits.max_payload_bytes {
        return Err(runtime_plan_error(
            "The Runtime plan payload exceeds its registered bounds.",
        ));
    }
    entries.sort_by(|left, right| left.key_digest.cmp(&right.key_digest));
    let parent = destination.parent().ok_or_else(|| {
        runtime_plan_error("The Runtime plan archive destination has no parent directory.")
    })?;
    tokio_fs::create_dir_all(parent)
        .await
        .map_err(|error| runtime_plan_io(format!("create Runtime plan archive parent: {error}")))?;
    let temporary_parent = parent.to_path_buf();
    let temporary = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".a3s-use-runtime-plans-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| runtime_plan_io(format!("join Runtime plan archive staging: {error}")))?
    .map_err(|error| runtime_plan_io(format!("create Runtime plan archive staging: {error}")))?;
    let writer_file = temporary
        .as_file()
        .try_clone()
        .map_err(|error| runtime_plan_io(format!("clone Runtime plan archive handle: {error}")))?;
    let mut writer = tokio_fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    for entry in &entries {
        let record = records
            .iter()
            .find(|record| {
                record.key.descriptor_digest().ok().as_deref() == Some(&entry.key_digest)
            })
            .ok_or_else(|| runtime_plan_error("Runtime plan archive inventory lost a record."))?;
        writer
            .write_all(&record.bytes)
            .await
            .map_err(|error| runtime_plan_io(format!("write Runtime plan archive: {error}")))?;
        archive_digest.update(&record.bytes);
    }
    writer
        .flush()
        .await
        .map_err(|error| runtime_plan_io(format!("flush Runtime plan archive: {error}")))?;
    writer
        .sync_all()
        .await
        .map_err(|error| runtime_plan_io(format!("sync Runtime plan archive: {error}")))?;
    drop(writer);
    let after = store
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_plan_error)?;
    if after != records {
        return Err(runtime_plan_error(
            "Runtime plan records changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| runtime_plan_io(format!("join Runtime plan archive publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            runtime_plan_error("The Runtime plan archive destination already exists.")
        } else {
            runtime_plan_io(format!("publish Runtime plan archive: {error}"))
        }
    })?;
    if let Some(parent) = destination.parent() {
        sync_directory(parent).await?;
    }
    Ok(CapturedRuntimePlans {
        payload: ControlRuntimePlanPayloadState::Archive {
            archive_bytes: total,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        entries,
        archive_path: Some(destination),
    })
}

pub(super) async fn verify_archive(
    snapshot: &ControlRuntimePlanPayloadSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<()> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlRuntimePlanPayloadState::Absent, None) => Ok(()),
        (ControlRuntimePlanPayloadState::Archive { .. }, Some(path)) => {
            let _ = read_archive_records(path, snapshot).await?;
            Ok(())
        }
        _ => Err(runtime_plan_error(
            "Runtime plan archive presence differs from its snapshot manifest.",
        )),
    }
}

pub(super) async fn read_archive_records(
    path: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<Vec<RuntimeSurfacePlanStoredRecord>> {
    let (mut file, before) = open_owned_file(path).await?;
    let expected_bytes = match snapshot.manifest.payload {
        ControlRuntimePlanPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlRuntimePlanPayloadState::Absent => 0,
    };
    if before.len() != expected_bytes {
        return Err(runtime_plan_error(
            "The Runtime plan archive length differs from its manifest.",
        ));
    }
    let mut digest = Sha256::new();
    let mut records = Vec::with_capacity(snapshot.manifest.entries.len());
    for entry in &snapshot.manifest.entries {
        let length = usize::try_from(entry.length)
            .map_err(|_| runtime_plan_error("Runtime plan archive record length overflowed."))?;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes)
            .await
            .map_err(|_| runtime_plan_error("The Runtime plan archive is truncated."))?;
        if digest_bytes(&bytes) != entry.sha256 {
            return Err(runtime_plan_error(
                "A Runtime plan archive record differs from its manifest digest.",
            ));
        }
        let (key, _) =
            RuntimeSurfacePlanStore::decode_record_bytes(&bytes).map_err(wrap_plan_error)?;
        if key != entry.key || key.descriptor_digest().map_err(wrap_plan_error)? != entry.key_digest
        {
            return Err(runtime_plan_error(
                "A Runtime plan archive record differs from its addressed key.",
            ));
        }
        digest.update(&bytes);
        records.push(RuntimeSurfacePlanStoredRecord { key, bytes });
    }
    let mut trailing = [0_u8; 1];
    if file
        .read(&mut trailing)
        .await
        .map_err(|error| runtime_plan_io(format!("read Runtime plan archive tail: {error}")))?
        != 0
    {
        return Err(runtime_plan_error(
            "The Runtime plan archive contains trailing bytes.",
        ));
    }
    if let ControlRuntimePlanPayloadState::Archive { archive_sha256, .. } =
        &snapshot.manifest.payload
    {
        if format!("sha256:{:x}", digest.finalize()) != *archive_sha256 {
            return Err(runtime_plan_error(
                "The Runtime plan archive digest differs from its manifest.",
            ));
        }
    }
    let after = file
        .metadata()
        .await
        .map_err(|error| runtime_plan_io(format!("reinspect Runtime plan archive: {error}")))?;
    if !after.is_file()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(runtime_plan_error(
            "The Runtime plan archive changed during offline verification.",
        ));
    }
    Ok(records)
}

pub(super) async fn stage_archive_file(
    source: &Path,
    staging: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<PathBuf> {
    let target = staging.join(ARCHIVE_FILE);
    let partial = staging.join(ARCHIVE_PARTIAL_FILE);
    let expected = read_archive_bytes(source, snapshot).await?;
    if let Some(existing) = optional_owned_file(&target).await? {
        if tokio_fs::read(&existing).await.map_err(|error| {
            runtime_plan_io(format!("read staged Runtime plan archive: {error}"))
        })? != expected
        {
            return Err(restore_invalid(
                "The staged Runtime plan archive differs from its exact snapshot.",
            ));
        }
        return Ok(target);
    }
    if let Some(existing) = optional_owned_file(&partial).await? {
        let bytes = tokio_fs::read(&existing).await.map_err(|error| {
            runtime_plan_io(format!("read partial Runtime plan archive: {error}"))
        })?;
        if bytes == expected {
            publish_noclobber(partial, target.clone()).await?;
            return Ok(target);
        }
        if bytes.len() >= expected.len() {
            return Err(restore_invalid(
                "The partial Runtime plan archive contains unexpected complete bytes.",
            ));
        }
        tokio_fs::remove_file(&partial).await.map_err(|error| {
            runtime_plan_io(format!("remove partial Runtime plan archive: {error}"))
        })?;
    }
    let mut file = tokio_fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| {
            runtime_plan_io(format!("create partial Runtime plan archive: {error}"))
        })?;
    file.write_all(&expected)
        .await
        .map_err(|error| runtime_plan_io(format!("write partial Runtime plan archive: {error}")))?;
    file.flush()
        .await
        .map_err(|error| runtime_plan_io(format!("flush partial Runtime plan archive: {error}")))?;
    file.sync_all()
        .await
        .map_err(|error| runtime_plan_io(format!("sync partial Runtime plan archive: {error}")))?;
    drop(file);
    publish_noclobber(partial, target.clone()).await?;
    Ok(target)
}

pub(super) async fn read_archive_bytes(
    source: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<Vec<u8>> {
    let bytes = tokio_fs::read(source)
        .await
        .map_err(|error| runtime_plan_io(format!("read Runtime plan archive source: {error}")))?;
    let expected = match snapshot.manifest.payload {
        ControlRuntimePlanPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlRuntimePlanPayloadState::Absent => 0,
    };
    if bytes.len() as u64 != expected {
        return Err(runtime_plan_error(
            "The Runtime plan archive source has unexpected length.",
        ));
    }
    Ok(bytes)
}
