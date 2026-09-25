//! Filesystem helpers for Capability payload candidate/live roots and staging.

use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::capability_catalog_store::{
    CapabilityGatewayCatalogStore, CapabilityGatewayCatalogStoredRecord,
};
use crate::control_store::effect_owner::capability_plane::{
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilityDescriptorSnapshotStoredRecord,
};

use super::{
    ACTIVATION_FILE, ACTIVATION_PARTIAL_FILE, ARCHIVE_FILE, ARCHIVE_PARTIAL_FILE,
    CANDIDATE_DIRECTORY, CATALOGS_DIRECTORY, DESCRIPTOR_SNAPSHOTS_DIRECTORY, MAX_ACTIVATION_BYTES,
    ControlCapabilityPayloadEntryKind, ControlCapabilityPayloadSnapshot,
};
use super::helpers::{
    capability_payload_error, capability_payload_io, digest_bytes, restore_invalid,
    restore_target_not_empty, wrap_capability_error,
};

pub(super) async fn validate_candidate(
    candidate: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<()> {
    validate_inventory_at(candidate, snapshot).await
}

pub(super) async fn validate_live_root(
    live: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<()> {
    validate_inventory_at(live, snapshot).await
}

pub(super) async fn validate_inventory_at(
    root: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<()> {
    let installation = &snapshot.manifest.binding.installation;
    let catalogs = CapabilityGatewayCatalogStore::inspect_records_at(
        &root.join(CATALOGS_DIRECTORY),
        installation,
    )
    .await
    .map_err(wrap_capability_error)?;
    let descriptors = ControlCapabilityDescriptorSnapshotStore::inspect_records_at(
        &root.join(DESCRIPTOR_SNAPSHOTS_DIRECTORY),
        installation,
    )
    .await
    .map_err(wrap_capability_error)?;
    let expected_catalogs = snapshot
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == ControlCapabilityPayloadEntryKind::Catalog)
        .collect::<Vec<_>>();
    let expected_descriptors = snapshot
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == ControlCapabilityPayloadEntryKind::DescriptorSnapshot)
        .collect::<Vec<_>>();
    if catalogs.len() != expected_catalogs.len()
        || descriptors.len() != expected_descriptors.len()
        || catalogs
            .iter()
            .zip(&expected_catalogs)
            .any(|(record, entry)| {
                record.digest != entry.digest
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
        || descriptors
            .iter()
            .zip(&expected_descriptors)
            .any(|(record, entry)| {
                record.digest != entry.digest
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
    {
        return Err(restore_invalid(
            "The Capability payload restore inventory differs from its exact snapshot.",
        ));
    }
    // Absent optional child directories are fine when that kind has no entries.
    if expected_catalogs.is_empty() {
        reject_unexpected_child(root, CATALOGS_DIRECTORY).await?;
    }
    if expected_descriptors.is_empty() {
        reject_unexpected_child(root, DESCRIPTOR_SNAPSHOTS_DIRECTORY).await?;
    }
    Ok(())
}

pub(super) async fn reject_unexpected_child(root: &Path, name: &str) -> UseResult<()> {
    let path = root.join(name);
    match tokio_fs::symlink_metadata(&path).await {
        Ok(_) => Err(restore_invalid(
            "The Capability payload root contains an unexpected empty child directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload child directory: {error}"
        ))),
    }
}

pub(super) async fn materialize_candidate(
    candidate: &Path,
    installation: &InstallationId,
    catalogs: &[CapabilityGatewayCatalogStoredRecord],
    descriptors: &[ControlCapabilityDescriptorSnapshotStoredRecord],
) -> UseResult<()> {
    validate_directory(candidate).await?;
    if !catalogs.is_empty() {
        let catalogs_root = candidate.join(CATALOGS_DIRECTORY);
        ensure_owned_directory(candidate, &catalogs_root).await?;
        CapabilityGatewayCatalogStore::materialize_records(&catalogs_root, installation, catalogs)
            .await
            .map_err(wrap_capability_error)?;
    }
    if !descriptors.is_empty() {
        let descriptors_root = candidate.join(DESCRIPTOR_SNAPSHOTS_DIRECTORY);
        ensure_owned_directory(candidate, &descriptors_root).await?;
        ControlCapabilityDescriptorSnapshotStore::materialize_records(
            &descriptors_root,
            installation,
            descriptors,
        )
        .await
        .map_err(wrap_capability_error)?;
    }
    Ok(())
}

pub(super) async fn inspect_live_root(state_root: &Path) -> UseResult<Option<PathBuf>> {
    let path = state_root.join(CANDIDATE_DIRECTORY);
    match tokio_fs::symlink_metadata(&path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(Some(path))
        }
        Ok(_) => Err(restore_invalid(
            "The live Capability payload root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(capability_payload_io(format!(
            "inspect live Capability payload root: {error}"
        ))),
    }
}

pub(super) async fn owned_directory(path: &Path) -> UseResult<bool> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(restore_invalid(
            "A Capability payload restore directory is not owned.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload restore directory: {error}"
        ))),
    }
}

pub(super) async fn open_owned_file(path: &Path) -> UseResult<(tokio_fs::File, std::fs::Metadata)> {
    let metadata = tokio_fs::symlink_metadata(path).await.map_err(|error| {
        capability_payload_io(format!("inspect Capability payload archive: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
    {
        return Err(capability_payload_error(
            "The Capability payload archive is not an owned regular file.",
        ));
    }
    let file = tokio_fs::File::open(path).await.map_err(|error| {
        capability_payload_io(format!("open Capability payload archive: {error}"))
    })?;
    Ok((file, metadata))
}

pub(super) async fn optional_owned_file(path: &Path) -> UseResult<Option<PathBuf>> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(Some(path.to_path_buf()))
        }
        Ok(_) => Err(restore_invalid(
            "A Capability payload restore archive path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload restore archive: {error}"
        ))),
    }
}

pub(super) async fn staged_archive(staging: &Path) -> UseResult<PathBuf> {
    optional_owned_file(&staging.join(ARCHIVE_FILE))
        .await?
        .ok_or_else(|| restore_invalid("The Capability payload restore archive is missing."))
}

pub(super) async fn validate_staging_entries(staging: &Path) -> UseResult<()> {
    validate_directory(staging).await?;
    let mut entries = tokio_fs::read_dir(staging).await.map_err(|error| {
        capability_payload_io(format!("read Capability payload restore staging: {error}"))
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        capability_payload_io(format!(
            "read Capability payload restore staging entry: {error}"
        ))
    })? {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Capability payload restore staging names must be valid UTF-8.")
        })?;
        let metadata = tokio_fs::symlink_metadata(entry.path()).await.map_err(|error| {
            capability_payload_io(format!(
                "inspect Capability payload restore staging entry: {error}"
            ))
        })?;
        let is_candidate = name == CANDIDATE_DIRECTORY;
        let is_file = matches!(
            name.as_str(),
            ARCHIVE_FILE | ARCHIVE_PARTIAL_FILE | ACTIVATION_FILE | ACTIVATION_PARTIAL_FILE
        );
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || (is_candidate && !metadata.is_dir())
            || (!is_candidate && (!is_file || !metadata.is_file()))
        {
            return Err(restore_invalid(
                "Capability payload restore staging contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

pub(super) async fn recover_activation_marker(staging: &Path, expected: &[u8]) -> UseResult<bool> {
    let marker = staging.join(ACTIVATION_FILE);
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker_length = optional_owned_file_length(&marker).await?;
    let partial_length = optional_owned_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The Capability payload activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_owned_file(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The Capability payload activation marker differs from its exact snapshot.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        tokio_fs::remove_file(&partial).await.map_err(|error| {
            capability_payload_io(format!(
                "remove incomplete Capability payload activation marker: {error}"
            ))
        })?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_owned_file(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A staged Capability payload activation marker has unexpected complete bytes.",
        ));
    }
    publish_noclobber(partial, marker.clone()).await?;
    sync_directory(staging).await?;
    Ok(true)
}

pub(super) async fn create_activation_marker(staging: &Path, expected: &[u8]) -> UseResult<()> {
    if recover_activation_marker(staging, expected).await? {
        return Ok(());
    }
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker = staging.join(ACTIVATION_FILE);
    let mut output = tokio_fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "create Capability payload activation marker: {error}"
            ))
        })?;
    output.write_all(expected).await.map_err(|error| {
        capability_payload_io(format!(
            "write Capability payload activation marker: {error}"
        ))
    })?;
    output.flush().await.map_err(|error| {
        capability_payload_io(format!(
            "flush Capability payload activation marker: {error}"
        ))
    })?;
    output.sync_all().await.map_err(|error| {
        capability_payload_io(format!(
            "sync Capability payload activation marker: {error}"
        ))
    })?;
    drop(output);
    if read_owned_file(&partial, expected.len() as u64).await? != expected {
        return Err(restore_invalid(
            "The Capability payload activation marker changed before publication.",
        ));
    }
    publish_noclobber(partial, marker).await?;
    sync_directory(staging).await
}

pub(super) async fn optional_owned_file_length(path: &Path) -> UseResult<Option<u64>> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            if metadata.len() == 0 || metadata.len() > MAX_ACTIVATION_BYTES {
                return Err(restore_invalid(
                    "The Capability payload activation marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "The Capability payload activation marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload activation marker: {error}"
        ))),
    }
}

pub(super) async fn read_owned_file(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    let bytes = tokio_fs::read(path).await.map_err(|error| {
        capability_payload_io(format!(
            "read Capability payload activation marker: {error}"
        ))
    })?;
    let metadata = tokio_fs::symlink_metadata(path).await.map_err(|error| {
        capability_payload_io(format!(
            "reinspect Capability payload activation marker: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != expected_length
        || bytes.len() as u64 != expected_length
    {
        return Err(restore_invalid(
            "The Capability payload activation marker changed while it was read.",
        ));
    }
    Ok(bytes)
}

pub(super) async fn require_empty_staging(staging: &Path) -> UseResult<()> {
    validate_staging_entries(staging).await?;
    let mut entries = tokio_fs::read_dir(staging).await.map_err(|error| {
        capability_payload_io(format!("read absent Capability payload staging: {error}"))
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "read absent Capability payload staging entry: {error}"
            ))
        })?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent Capability payload snapshot has unexpected staged state.",
        ));
    }
    Ok(())
}

pub(super) async fn ensure_owned_directory(root: &Path, target: &Path) -> UseResult<()> {
    if target == root || !target.starts_with(root) {
        return Err(restore_invalid(
            "A Capability payload restore directory escapes its state root.",
        ));
    }
    validate_directory(root).await?;
    let relative = target.strip_prefix(root).map_err(|_| {
        restore_invalid("A Capability payload restore directory is not state-owned.")
    })?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Capability payload restore directory is not normalized.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match tokio_fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(capability_payload_io(format!(
                    "create Capability payload restore directory: {error}"
                )))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = tokio_fs::symlink_metadata(path).await.map_err(|error| {
        capability_payload_io(format!("inspect Capability payload directory: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Capability payload restore path is not an owned directory.",
        ));
    }
    Ok(())
}

pub(super) fn validate_staging_location(state_root: &Path, staging: &Path) -> UseResult<()> {
    if staging == state_root || !staging.starts_with(state_root) {
        return Err(restore_invalid(
            "Capability payload restore staging escapes the target state root.",
        ));
    }
    let relative = staging
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("Capability payload restore staging is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "Capability payload restore staging is not normalized.",
        ));
    }
    if staging.starts_with(state_root.join(CANDIDATE_DIRECTORY)) {
        return Err(restore_invalid(
            "The Capability payload candidate cannot be staged inside its live root.",
        ));
    }
    Ok(())
}

pub(super) async fn publish_directory(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let target_for_worker = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target_for_worker)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!("join Capability payload root publication: {error}"))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            capability_payload_io(format!("publish Capability payload root: {error}"))
        }
    })?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

pub(super) async fn publish_noclobber(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let target_for_worker = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target_for_worker)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!(
            "join Capability payload archive publication: {error}"
        ))
    })?
    .map_err(|error| {
        capability_payload_io(format!("publish Capability payload archive: {error}"))
    })?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

pub(super) async fn path_exists(path: &Path) -> UseResult<bool> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload path: {error}"
        ))),
    }
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    tokio_fs::File::open(path)
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "open Capability payload directory for sync: {error}"
            ))
        })?
        .sync_all()
        .await
        .map_err(|error| {
            capability_payload_io(format!("sync Capability payload directory: {error}"))
        })
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
