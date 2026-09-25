//! Archive encoding, verification, and staging for Capability payload snapshots.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;
use sha2::{Digest, Sha256};
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::capability_catalog_store::{
    CapabilityGatewayCatalogStore, CapabilityGatewayCatalogStoredRecord,
};
use crate::control_store::effect_owner::capability_plane::{
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilityDescriptorSnapshotStoredRecord,
};
use crate::control_store::model::valid_sha256;

use super::super::ControlPayloadOwnerLimits;
use super::{
    ARCHIVE_FILE, ARCHIVE_PARTIAL_FILE, ControlCapabilityPayloadEntry,
    ControlCapabilityPayloadEntryKind, ControlCapabilityPayloadSnapshot,
    ControlCapabilityPayloadState, MAX_ARCHIVE_RECORD_BYTES,
};
use super::filesystem::{
    open_owned_file, optional_owned_file, path_exists, publish_noclobber, sync_directory,
};
use super::helpers::{
    capability_payload_error, capability_payload_io, digest_bytes, restore_invalid,
    wrap_capability_error,
};
pub(super) struct CapturedCapabilityPayload {
    pub(super) payload: ControlCapabilityPayloadState,
    pub(super) entries: Vec<ControlCapabilityPayloadEntry>,
    pub(super) archive_path: Option<PathBuf>,
}

pub(super) async fn snapshot_live(
    catalogs: &CapabilityGatewayCatalogStore,
    descriptors: &ControlCapabilityDescriptorSnapshotStore,
    maintenance: &StateMaintenanceGuard,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<CapturedCapabilityPayload> {
    let catalog_records = catalogs
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    let descriptor_records = descriptors
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    let mut entries = Vec::with_capacity(catalog_records.len() + descriptor_records.len());
    let mut total = 0_u64;
    for record in &catalog_records {
        let length = u64::try_from(record.bytes.len()).map_err(|_| {
            capability_payload_error("Capability payload record length overflowed.")
        })?;
        total = total.checked_add(length).ok_or_else(|| {
            capability_payload_error("Capability payload archive byte accounting overflowed.")
        })?;
        entries.push(ControlCapabilityPayloadEntry {
            kind: ControlCapabilityPayloadEntryKind::Catalog,
            digest: record.digest.clone(),
            length,
            sha256: digest_bytes(&record.bytes),
        });
    }
    for record in &descriptor_records {
        let length = u64::try_from(record.bytes.len()).map_err(|_| {
            capability_payload_error("Capability payload record length overflowed.")
        })?;
        total = total.checked_add(length).ok_or_else(|| {
            capability_payload_error("Capability payload archive byte accounting overflowed.")
        })?;
        entries.push(ControlCapabilityPayloadEntry {
            kind: ControlCapabilityPayloadEntryKind::DescriptorSnapshot,
            digest: record.digest.clone(),
            length,
            sha256: digest_bytes(&record.bytes),
        });
    }
    if entries.is_empty() {
        if path_exists(&destination).await? {
            return Err(capability_payload_error(
                "An absent Capability payload has an unexpected archive file.",
            ));
        }
        return Ok(CapturedCapabilityPayload {
            payload: ControlCapabilityPayloadState::Absent,
            entries: Vec::new(),
            archive_path: None,
        });
    }
    if entries.len() as u64 > limits.max_files || total > limits.max_payload_bytes {
        return Err(capability_payload_error(
            "The Capability payload exceeds its registered bounds.",
        ));
    }
    entries.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    let parent = destination.parent().ok_or_else(|| {
        capability_payload_error(
            "The Capability payload archive destination has no parent directory.",
        )
    })?;
    tokio_fs::create_dir_all(parent).await.map_err(|error| {
        capability_payload_io(format!("create Capability payload archive parent: {error}"))
    })?;
    let temporary_parent = parent.to_path_buf();
    let temporary = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".a3s-use-capability-payload-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!("join Capability payload archive staging: {error}"))
    })?
    .map_err(|error| {
        capability_payload_io(format!(
            "create Capability payload archive staging: {error}"
        ))
    })?;
    let writer_file = temporary.as_file().try_clone().map_err(|error| {
        capability_payload_io(format!("clone Capability payload archive handle: {error}"))
    })?;
    let mut writer = tokio_fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    let record_bytes = |kind: ControlCapabilityPayloadEntryKind,
                        digest: &str|
     -> UseResult<&[u8]> {
        match kind {
            ControlCapabilityPayloadEntryKind::Catalog => catalog_records
                .iter()
                .find(|record| record.digest == digest)
                .map(|record| record.bytes.as_slice())
                .ok_or_else(|| {
                    capability_payload_error("Capability payload archive inventory lost a catalog.")
                }),
            ControlCapabilityPayloadEntryKind::DescriptorSnapshot => descriptor_records
                .iter()
                .find(|record| record.digest == digest)
                .map(|record| record.bytes.as_slice())
                .ok_or_else(|| {
                    capability_payload_error(
                        "Capability payload archive inventory lost a descriptor snapshot.",
                    )
                }),
        }
    };
    for entry in &entries {
        let bytes = record_bytes(entry.kind, &entry.digest)?;
        writer.write_all(bytes).await.map_err(|error| {
            capability_payload_io(format!("write Capability payload archive: {error}"))
        })?;
        archive_digest.update(bytes);
    }
    writer.flush().await.map_err(|error| {
        capability_payload_io(format!("flush Capability payload archive: {error}"))
    })?;
    writer.sync_all().await.map_err(|error| {
        capability_payload_io(format!("sync Capability payload archive: {error}"))
    })?;
    drop(writer);
    let after_catalogs = catalogs
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    let after_descriptors = descriptors
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    if after_catalogs != catalog_records || after_descriptors != descriptor_records {
        return Err(capability_payload_error(
            "Capability payload records changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!(
            "join Capability payload archive publication: {error}"
        ))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            capability_payload_error("The Capability payload archive destination already exists.")
        } else {
            capability_payload_io(format!("publish Capability payload archive: {error}"))
        }
    })?;
    if let Some(parent) = destination.parent() {
        sync_directory(parent).await?;
    }
    Ok(CapturedCapabilityPayload {
        payload: ControlCapabilityPayloadState::Archive {
            archive_bytes: total,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        entries,
        archive_path: Some(destination),
    })
}

pub(super) async fn verify_archive(
    snapshot: &ControlCapabilityPayloadSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<()> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlCapabilityPayloadState::Absent, None) => Ok(()),
        (ControlCapabilityPayloadState::Archive { .. }, Some(path)) => {
            let _ = read_archive_records(path, snapshot).await?;
            Ok(())
        }
        _ => Err(capability_payload_error(
            "Capability payload archive presence differs from its snapshot manifest.",
        )),
    }
}

pub(super) async fn read_archive_records(
    path: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<(
    Vec<CapabilityGatewayCatalogStoredRecord>,
    Vec<ControlCapabilityDescriptorSnapshotStoredRecord>,
)> {
    let (mut file, before) = open_owned_file(path).await?;
    let expected_bytes = match snapshot.manifest.payload {
        ControlCapabilityPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlCapabilityPayloadState::Absent => 0,
    };
    if before.len() != expected_bytes {
        return Err(capability_payload_error(
            "The Capability payload archive length differs from its manifest.",
        ));
    }
    let mut digest = Sha256::new();
    let mut catalogs = Vec::new();
    let mut descriptors = Vec::new();
    for entry in &snapshot.manifest.entries {
        let length = usize::try_from(entry.length).map_err(|_| {
            capability_payload_error("Capability payload archive record length overflowed.")
        })?;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes).await.map_err(|_| {
            capability_payload_error("The Capability payload archive is truncated.")
        })?;
        if digest_bytes(&bytes) != entry.sha256 {
            return Err(capability_payload_error(
                "A Capability payload archive record differs from its manifest digest.",
            ));
        }
        digest.update(&bytes);
        match entry.kind {
            ControlCapabilityPayloadEntryKind::Catalog => {
                catalogs.push(CapabilityGatewayCatalogStoredRecord {
                    digest: entry.digest.clone(),
                    bytes,
                });
            }
            ControlCapabilityPayloadEntryKind::DescriptorSnapshot => {
                descriptors.push(ControlCapabilityDescriptorSnapshotStoredRecord {
                    digest: entry.digest.clone(),
                    bytes,
                });
            }
        }
    }
    let mut trailing = [0_u8; 1];
    if file.read(&mut trailing).await.map_err(|error| {
        capability_payload_io(format!("read Capability payload archive tail: {error}"))
    })? != 0
    {
        return Err(capability_payload_error(
            "The Capability payload archive contains trailing bytes.",
        ));
    }
    if let ControlCapabilityPayloadState::Archive { archive_sha256, .. } =
        &snapshot.manifest.payload
    {
        if format!("sha256:{:x}", digest.finalize()) != *archive_sha256 {
            return Err(capability_payload_error(
                "The Capability payload archive digest differs from its manifest.",
            ));
        }
    }
    let after = file.metadata().await.map_err(|error| {
        capability_payload_io(format!("reinspect Capability payload archive: {error}"))
    })?;
    if !after.is_file()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(capability_payload_error(
            "The Capability payload archive changed during offline verification.",
        ));
    }
    Ok((catalogs, descriptors))
}

pub(super) async fn stage_archive_file(
    source: &Path,
    staging: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<PathBuf> {
    let target = staging.join(ARCHIVE_FILE);
    let partial = staging.join(ARCHIVE_PARTIAL_FILE);
    let expected = read_archive_bytes(source, snapshot).await?;
    if let Some(existing) = optional_owned_file(&target).await? {
        if tokio_fs::read(&existing).await.map_err(|error| {
            capability_payload_io(format!("read staged Capability payload archive: {error}"))
        })? != expected
        {
            return Err(restore_invalid(
                "The staged Capability payload archive differs from its exact snapshot.",
            ));
        }
        return Ok(target);
    }
    if let Some(existing) = optional_owned_file(&partial).await? {
        let bytes = tokio_fs::read(&existing).await.map_err(|error| {
            capability_payload_io(format!("read partial Capability payload archive: {error}"))
        })?;
        if bytes == expected {
            publish_noclobber(partial, target.clone()).await?;
            return Ok(target);
        }
        if bytes.len() >= expected.len() {
            return Err(restore_invalid(
                "The partial Capability payload archive contains unexpected complete bytes.",
            ));
        }
        tokio_fs::remove_file(&partial).await.map_err(|error| {
            capability_payload_io(format!(
                "remove partial Capability payload archive: {error}"
            ))
        })?;
    }
    let mut file = tokio_fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "create partial Capability payload archive: {error}"
            ))
        })?;
    file.write_all(&expected).await.map_err(|error| {
        capability_payload_io(format!("write partial Capability payload archive: {error}"))
    })?;
    file.flush().await.map_err(|error| {
        capability_payload_io(format!("flush partial Capability payload archive: {error}"))
    })?;
    file.sync_all().await.map_err(|error| {
        capability_payload_io(format!("sync partial Capability payload archive: {error}"))
    })?;
    drop(file);
    publish_noclobber(partial, target.clone()).await?;
    Ok(target)
}

pub(super) async fn read_archive_bytes(
    source: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<Vec<u8>> {
    let bytes = tokio_fs::read(source).await.map_err(|error| {
        capability_payload_io(format!("read Capability payload archive source: {error}"))
    })?;
    let expected = match snapshot.manifest.payload {
        ControlCapabilityPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlCapabilityPayloadState::Absent => 0,
    };
    if bytes.len() as u64 != expected {
        return Err(capability_payload_error(
            "The Capability payload archive source has unexpected length.",
        ));
    }
    Ok(bytes)
}
