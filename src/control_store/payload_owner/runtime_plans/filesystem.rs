//! Filesystem helpers for Runtime plan payload candidate/live roots and staging.

use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use tokio::fs as tokio_fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::plugin_runtime::RuntimeSurfacePlanStore;

use super::{
    candidate_path, ACTIVATION_FILE, ACTIVATION_PARTIAL_FILE, ARCHIVE_FILE, ARCHIVE_PARTIAL_FILE,
    CANDIDATE_DIRECTORY, MAX_ACTIVATION_BYTES, ControlRuntimePlanPayloadSnapshot,
};
use super::helpers::{
    digest_bytes, restore_invalid, restore_target_not_empty, runtime_plan_error, runtime_plan_io,
    wrap_plan_error,
};


pub(super) async fn validate_candidate(
    candidate: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<()> {
    let records = RuntimeSurfacePlanStore::inspect_exact_records_at(
        candidate,
        &snapshot.manifest.binding.installation,
    )
    .await
    .map_err(wrap_plan_error)?;
    if records.len() != snapshot.manifest.entries.len()
        || records
            .iter()
            .zip(&snapshot.manifest.entries)
            .any(|(record, entry)| {
                record.key != entry.key
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
    {
        return Err(restore_invalid(
            "The Runtime plan restore candidate differs from its exact snapshot inventory.",
        ));
    }
    Ok(())
}

pub(super) async fn validate_live_root(
    live: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<()> {
    let records =
        RuntimeSurfacePlanStore::inspect_records_at(live, &snapshot.manifest.binding.installation)
            .await
            .map_err(wrap_plan_error)?;
    if records.len() != snapshot.manifest.entries.len()
        || records
            .iter()
            .zip(&snapshot.manifest.entries)
            .any(|(record, entry)| {
                record.key != entry.key
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
    {
        return Err(restore_invalid(
            "The live Runtime plan root differs from its exact snapshot inventory.",
        ));
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
            "The live Runtime plan root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect live Runtime plan root: {error}"
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
            "A Runtime plan restore directory is not owned.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan restore directory: {error}"
        ))),
    }
}

pub(super) async fn open_owned_file(path: &Path) -> UseResult<(tokio_fs::File, std::fs::Metadata)> {
    let metadata = tokio_fs::symlink_metadata(path)
        .await
        .map_err(|error| runtime_plan_io(format!("inspect Runtime plan archive: {error}")))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
    {
        return Err(runtime_plan_error(
            "The Runtime plan archive is not an owned regular file.",
        ));
    }
    let file = tokio_fs::File::open(path)
        .await
        .map_err(|error| runtime_plan_io(format!("open Runtime plan archive: {error}")))?;
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
            "A Runtime plan restore archive path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan restore archive: {error}"
        ))),
    }
}

pub(super) async fn staged_archive(staging: &Path) -> UseResult<PathBuf> {
    optional_owned_file(&staging.join(ARCHIVE_FILE))
        .await?
        .ok_or_else(|| restore_invalid("The Runtime plan restore archive is missing."))
}

pub(super) async fn validate_staging_entries(staging: &Path) -> UseResult<()> {
    validate_directory(staging).await?;
    let mut entries = tokio_fs::read_dir(staging)
        .await
        .map_err(|error| runtime_plan_io(format!("read Runtime plan restore staging: {error}")))?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        runtime_plan_io(format!("read Runtime plan restore staging entry: {error}"))
    })? {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Runtime plan restore staging names must be valid UTF-8.")
        })?;
        let metadata = tokio_fs::symlink_metadata(entry.path()).await.map_err(|error| {
            runtime_plan_io(format!(
                "inspect Runtime plan restore staging entry: {error}"
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
                "Runtime plan restore staging contains an unowned entry.",
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
            "The Runtime plan activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_owned_file(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The Runtime plan activation marker differs from its exact snapshot.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        tokio_fs::remove_file(&partial).await.map_err(|error| {
            runtime_plan_io(format!(
                "remove incomplete Runtime plan activation marker: {error}"
            ))
        })?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_owned_file(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A staged Runtime plan activation marker has unexpected complete bytes.",
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
            runtime_plan_io(format!("create Runtime plan activation marker: {error}"))
        })?;
    output.write_all(expected).await.map_err(|error| {
        runtime_plan_io(format!("write Runtime plan activation marker: {error}"))
    })?;
    output.flush().await.map_err(|error| {
        runtime_plan_io(format!("flush Runtime plan activation marker: {error}"))
    })?;
    output.sync_all().await.map_err(|error| {
        runtime_plan_io(format!("sync Runtime plan activation marker: {error}"))
    })?;
    drop(output);
    if read_owned_file(&partial, expected.len() as u64).await? != expected {
        return Err(restore_invalid(
            "The Runtime plan activation marker changed before publication.",
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
                    "The Runtime plan activation marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "The Runtime plan activation marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan activation marker: {error}"
        ))),
    }
}

pub(super) async fn read_owned_file(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    let bytes = tokio_fs::read(path).await.map_err(|error| {
        runtime_plan_io(format!("read Runtime plan activation marker: {error}"))
    })?;
    let metadata = tokio_fs::symlink_metadata(path).await.map_err(|error| {
        runtime_plan_io(format!("reinspect Runtime plan activation marker: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != expected_length
        || bytes.len() as u64 != expected_length
    {
        return Err(restore_invalid(
            "The Runtime plan activation marker changed while it was read.",
        ));
    }
    Ok(bytes)
}

pub(super) async fn require_empty_staging(staging: &Path) -> UseResult<()> {
    validate_staging_entries(staging).await?;
    let mut entries = tokio_fs::read_dir(staging)
        .await
        .map_err(|error| runtime_plan_io(format!("read absent Runtime plan staging: {error}")))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| {
            runtime_plan_io(format!("read absent Runtime plan staging entry: {error}"))
        })?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent Runtime plan snapshot has unexpected staged state.",
        ));
    }
    Ok(())
}

pub(super) async fn ensure_owned_directory(root: &Path, target: &Path) -> UseResult<()> {
    if target == root || !target.starts_with(root) {
        return Err(restore_invalid(
            "A Runtime plan restore directory escapes its state root.",
        ));
    }
    validate_directory(root).await?;
    let relative = target
        .strip_prefix(root)
        .map_err(|_| restore_invalid("A Runtime plan restore directory is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Runtime plan restore directory is not normalized.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match tokio_fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(runtime_plan_io(format!(
                    "create Runtime plan restore directory: {error}"
                )))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = tokio_fs::symlink_metadata(path)
        .await
        .map_err(|error| runtime_plan_io(format!("inspect Runtime plan directory: {error}")))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Runtime plan restore path is not an owned directory.",
        ));
    }
    Ok(())
}

pub(super) fn validate_staging_location(state_root: &Path, staging: &Path) -> UseResult<()> {
    if staging == state_root || !staging.starts_with(state_root) {
        return Err(restore_invalid(
            "Runtime plan restore staging escapes the target state root.",
        ));
    }
    let relative = staging
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("Runtime plan restore staging is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "Runtime plan restore staging is not normalized.",
        ));
    }
    if staging.starts_with(state_root.join(CANDIDATE_DIRECTORY)) {
        return Err(restore_invalid(
            "The Runtime plan candidate cannot be staged inside its live root.",
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
    .map_err(|error| runtime_plan_io(format!("join Runtime plan root publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            runtime_plan_io(format!("publish Runtime plan root: {error}"))
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
    .map_err(|error| runtime_plan_io(format!("join Runtime plan archive publication: {error}")))?
    .map_err(|error| runtime_plan_io(format!("publish Runtime plan archive: {error}")))?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

pub(super) async fn path_exists(path: &Path) -> UseResult<bool> {
    match tokio_fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan path: {error}"
        ))),
    }
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    tokio_fs::File::open(path)
        .await
        .map_err(|error| runtime_plan_io(format!("open Runtime plan directory for sync: {error}")))?
        .sync_all()
        .await
        .map_err(|error| runtime_plan_io(format!("sync Runtime plan directory: {error}")))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
