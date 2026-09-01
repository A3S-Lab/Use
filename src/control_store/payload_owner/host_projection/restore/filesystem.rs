use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{restore_invalid, restore_target_not_empty};
use crate::control_store::payload_owner::host_projection::{
    archive, ControlHostProjectionSnapshot, ControlPayloadOwnerLimits,
};

const ARCHIVE_FILE: &str = "control-host-projection.archive";
const ARCHIVE_PARTIAL_FILE: &str = "control-host-projection.archive.partial";
const ACTIVATION_FILE: &str = "control-host-projection.activating.json";
const ACTIVATION_PARTIAL_FILE: &str = "control-host-projection.activating.json.partial";
const CANDIDATE_DIRECTORY: &str = "plugin-host-manager";

mod candidate;
mod staging_archive;

pub(super) use candidate::CanonicalHostProjection;
pub(super) use staging_archive::{stage_archive, staged_archive};

pub(super) enum LiveHostRoot {
    Absent,
    Owned(PathBuf),
}

pub(super) fn candidate_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(CANDIDATE_DIRECTORY)
}

pub(super) fn validate_staging_location(
    state_root: &Path,
    staging_directory: &Path,
) -> UseResult<()> {
    if staging_directory == state_root || !staging_directory.starts_with(state_root) {
        return Err(restore_invalid(
            "The Host projection restore staging directory escapes the target state root.",
        ));
    }
    let relative = staging_directory.strip_prefix(state_root).map_err(|_| {
        restore_invalid("The Host projection restore staging path is not state-owned.")
    })?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "The Host projection restore staging directory is not a normalized owned path.",
        ));
    }
    if staging_directory.starts_with(state_root.join(CANDIDATE_DIRECTORY)) {
        return Err(restore_invalid(
            "The Host projection restore candidate cannot be staged inside the live owner root.",
        ));
    }
    Ok(())
}

pub(super) async fn ensure_owned_directory(state_root: &Path, target: &Path) -> UseResult<()> {
    if target == state_root || !target.starts_with(state_root) {
        return Err(restore_invalid(
            "A Host projection restore directory escapes its state root.",
        ));
    }
    validate_directory(state_root).await?;
    let relative = target
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("A Host projection restore directory is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Host projection restore directory is not a normalized owned path.",
        ));
    }
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let created = match fs::create_dir(&current).await {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(restore_io("create Host restore directory", error)),
        };
        if created {
            let parent = current.parent().ok_or_else(|| {
                restore_invalid("A Host projection restore directory has no owned parent.")
            })?;
            sync_directory(parent).await?;
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_staging_entries(staging_directory: &Path) -> UseResult<()> {
    validate_directory(staging_directory).await?;
    let allowed_files = BTreeSet::from([
        ARCHIVE_FILE,
        ARCHIVE_PARTIAL_FILE,
        ACTIVATION_FILE,
        ACTIVATION_PARTIAL_FILE,
    ]);
    let mut entries = fs::read_dir(staging_directory)
        .await
        .map_err(|error| restore_io("read Host restore staging directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read Host restore staging entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Host projection restore staging names must be valid UTF-8.")
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_io("inspect Host restore staging entry", error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || name == CANDIDATE_DIRECTORY && !metadata.is_dir()
            || name != CANDIDATE_DIRECTORY
                && (!allowed_files.contains(name.as_str()) || !metadata.is_file())
        {
            return Err(restore_invalid(
                "The Host projection restore staging directory contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

pub(super) async fn recover_activation_marker(
    staging_directory: &Path,
    expected: &[u8],
) -> UseResult<bool> {
    let marker = staging_directory.join(ACTIVATION_FILE);
    let partial = staging_directory.join(ACTIVATION_PARTIAL_FILE);
    let marker_length = optional_regular_file_length(&marker).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The Host projection activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_exact_owned(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The Host projection activation marker differs from its exact snapshot.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete Host activation marker", error))?;
        sync_directory(staging_directory).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_exact_owned(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A staged Host projection activation marker has unexpected complete bytes.",
        ));
    }
    publish_noclobber(
        partial,
        marker,
        "publish Host projection activation marker",
        false,
    )
    .await?;
    sync_directory(staging_directory).await?;
    Ok(true)
}

pub(super) async fn require_absent_staging(staging_directory: &Path) -> UseResult<()> {
    validate_staging_entries(staging_directory).await?;
    let mut entries = fs::read_dir(staging_directory)
        .await
        .map_err(|error| restore_io("read absent Host restore staging directory", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read absent Host restore staging entry", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent Host projection snapshot has unexpected staged state.",
        ));
    }
    Ok(())
}

pub(super) async fn prepare_candidate(
    archive_path: &Path,
    staging_directory: &Path,
    snapshot: &ControlHostProjectionSnapshot,
    records: &[crate::cognitive_package::HostProjectionSnapshotRecord],
    limits: ControlPayloadOwnerLimits,
    build_if_missing: bool,
) -> UseResult<CanonicalHostProjection> {
    candidate::prepare(
        archive_path,
        staging_directory,
        snapshot,
        records,
        limits,
        build_if_missing,
    )
    .await
}

pub(super) async fn inspect_live_root(state_root: &Path) -> UseResult<LiveHostRoot> {
    let root = state_root.join(CANDIDATE_DIRECTORY);
    match fs::symlink_metadata(&root).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(LiveHostRoot::Owned(root))
        }
        Ok(_) => Err(restore_invalid(
            "The live Host projection root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(LiveHostRoot::Absent),
        Err(error) => Err(restore_io("inspect live Host projection root", error)),
    }
}

pub(super) async fn validate_candidate(
    staging_directory: &Path,
    snapshot: &ControlHostProjectionSnapshot,
    records: &[crate::cognitive_package::HostProjectionSnapshotRecord],
    canonical: &CanonicalHostProjection,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<()> {
    candidate::validate_projection_root(staging_directory, snapshot, records, canonical, limits)
        .await
}

#[allow(clippy::too_many_arguments)]
pub(super) async fn activate_candidate(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlHostProjectionSnapshot,
    records: &[crate::cognitive_package::HostProjectionSnapshotRecord],
    canonical: &CanonicalHostProjection,
    limits: ControlPayloadOwnerLimits,
    activation_started: bool,
    activation_bytes: &[u8],
) -> UseResult<()> {
    let candidate = candidate_path(staging_directory);
    let candidate_exists = optional_owned_directory(&candidate).await?;
    let live = inspect_live_root(state_root).await?;
    match (activation_started, candidate_exists, live) {
        (false, true, LiveHostRoot::Absent) => {
            candidate::validate_projection_root(
                staging_directory,
                snapshot,
                records,
                canonical,
                limits,
            )
            .await?;
            create_activation_marker(staging_directory, activation_bytes).await?;
            candidate::validate_projection_root(
                staging_directory,
                snapshot,
                records,
                canonical,
                limits,
            )
            .await?;
            if !matches!(inspect_live_root(state_root).await?, LiveHostRoot::Absent) {
                return Err(restore_target_not_empty());
            }
            publish_owner_root(candidate, state_root.join(CANDIDATE_DIRECTORY)).await?;
        }
        (true, true, LiveHostRoot::Absent) => {
            candidate::validate_projection_root(
                staging_directory,
                snapshot,
                records,
                canonical,
                limits,
            )
            .await?;
            publish_owner_root(candidate, state_root.join(CANDIDATE_DIRECTORY)).await?;
        }
        (true, false, LiveHostRoot::Owned(_)) => {}
        (false, _, LiveHostRoot::Owned(_)) | (true, true, LiveHostRoot::Owned(_)) => {
            return Err(restore_target_not_empty())
        }
        (false, false, LiveHostRoot::Absent) | (true, false, LiveHostRoot::Absent) => {
            return Err(restore_invalid(
                "The Host projection restore candidate disappeared before activation.",
            ))
        }
    }
    match inspect_live_root(state_root).await? {
        LiveHostRoot::Owned(path) if path == state_root.join(CANDIDATE_DIRECTORY) => {
            candidate::validate_projection_root(state_root, snapshot, records, canonical, limits)
                .await?
        }
        _ => {
            return Err(restore_invalid(
                "The activated Host projection root differs from its exact target.",
            ))
        }
    }
    if !recover_activation_marker(staging_directory, activation_bytes).await? {
        return Err(restore_invalid(
            "The Host projection activation marker disappeared before restore completion.",
        ));
    }
    Ok(())
}

async fn create_activation_marker(staging_directory: &Path, bytes: &[u8]) -> UseResult<()> {
    if recover_activation_marker(staging_directory, bytes).await? {
        return Ok(());
    }
    let partial = staging_directory.join(ACTIVATION_PARTIAL_FILE);
    let marker = staging_directory.join(ACTIVATION_FILE);
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create Host activation marker", error))?;
    output
        .write_all(bytes)
        .await
        .map_err(|error| restore_io("write Host activation marker", error))?;
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush Host activation marker", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Host activation marker", error))?;
    drop(output);
    if read_exact_owned(&partial, bytes.len() as u64).await? != bytes {
        return Err(restore_invalid(
            "The Host projection activation marker changed before publication.",
        ));
    }
    publish_noclobber(partial, marker, "publish Host activation marker", false).await?;
    sync_directory(staging_directory).await
}

async fn publish_owner_root(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let target_for_worker = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target_for_worker)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join Host root publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            restore_invalid(format!(
                "Failed to atomically publish the Host projection owner root '{}': {error}",
                target.display()
            ))
        }
    })?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

pub(super) async fn optional_owned_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(restore_invalid(
            "A Host projection restore candidate is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(restore_io("inspect Host restore candidate", error)),
    }
}

pub(super) async fn optional_regular_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "A Host projection restore file is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io("inspect Host restore file", error)),
    }
}

pub(super) async fn read_exact_owned(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    let (mut file, metadata) = archive::open_owned_regular_file(path, "Host restore file")
        .await
        .map_err(wrap_archive_error)?;
    if metadata.len() != expected_length {
        return Err(restore_invalid(
            "A Host projection restore file changed before it was read.",
        ));
    }
    let capacity = usize::try_from(expected_length)
        .map_err(|_| restore_invalid("A Host projection restore file length is invalid."))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_io("read Host projection restore file", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(restore_invalid(
            "A Host projection restore file changed while it was read.",
        ));
    }
    Ok(bytes)
}

pub(super) async fn publish_noclobber(
    source: PathBuf,
    target: PathBuf,
    action: &'static str,
    existing_is_live_payload: bool,
) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join {action}: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists && existing_is_live_payload {
            restore_target_not_empty()
        } else {
            restore_invalid(format!(
                "Failed to {action} '{}': {error}",
                error_target.display()
            ))
        }
    })
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect Host restore directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Host projection restore directory is not an owned directory.",
        ));
    }
    Ok(())
}

pub(super) fn wrap_archive_error(error: a3s_use_core::UseError) -> a3s_use_core::UseError {
    restore_invalid(format!(
        "Host projection restore archive verification failed: {}",
        error.message
    ))
}

pub(super) fn restore_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    restore_invalid(format!("Failed to {action}: {error}"))
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_io("open Host restore directory", error))?
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Host restore directory", error))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
