use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::AsyncReadExt;

use super::super::{ControlRestoreCoordinatorSnapshot, ControlRestoreCoordinatorState};
use super::evidence::RestoreCoordinatorActivation;
use super::restore_invalid;

const CANDIDATE_DIRECTORY: &str = "restore-history-candidate";
const RETIRED_DIRECTORY: &str = "retired";
const PUBLISHING_DIRECTORY: &str = "publishing";
const ACTIVATION_FILE: &str = "control-restore-coordinator.activating.json";
const ACTIVATION_PARTIAL_FILE: &str = "control-restore-coordinator.activating.json.partial";
const OPERATION_FILE: &str = "operation.json";
const OPERATION_PARTIAL_FILE: &str = "operation.json.partial";
pub(super) const MAX_ACTIVE_MARKER_BYTES: u64 = 4 * 1024;

pub(super) struct ExpectedActiveRestore<'a> {
    pub(super) plan_digest: &'a str,
    pub(super) marker_length: u64,
    pub(super) marker_sha256: &'a str,
}

mod candidate;
mod reconcile;
mod records;

pub(super) use candidate::CanonicalRestoreHistory;

pub(super) fn candidate_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(CANDIDATE_DIRECTORY)
}

pub(super) fn retired_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(RETIRED_DIRECTORY)
}

pub(super) fn publishing_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(PUBLISHING_DIRECTORY)
}

pub(super) fn activation_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(ACTIVATION_FILE)
}

pub(super) fn activation_partial_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(ACTIVATION_PARTIAL_FILE)
}

pub(super) fn validate_staging_location(
    state_root: &Path,
    staging_directory: &Path,
) -> UseResult<()> {
    if staging_directory == state_root || !staging_directory.starts_with(state_root) {
        return Err(restore_invalid(
            "The Restore Coordinator staging directory escapes the target state root.",
        ));
    }
    let relative = staging_directory
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("The Restore Coordinator staging path is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "The Restore Coordinator staging directory is not a normalized owned path.",
        ));
    }
    let live_root = state_root.join("operations").join("state-restores");
    if staging_directory.starts_with(&live_root) || live_root.starts_with(staging_directory) {
        return Err(restore_invalid(
            "Restore Coordinator staging cannot overlap the live restore history root.",
        ));
    }
    Ok(())
}

pub(super) async fn ensure_owned_directory(state_root: &Path, target: &Path) -> UseResult<()> {
    if target == state_root || !target.starts_with(state_root) {
        return Err(restore_invalid(
            "A Restore Coordinator directory escapes its owned root.",
        ));
    }
    validate_directory(state_root).await?;
    let relative = target
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("A Restore Coordinator directory is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Restore Coordinator directory is not a normalized owned path.",
        ));
    }
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        let created = match fs::create_dir(&current).await {
            Ok(()) => true,
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => false,
            Err(error) => return Err(restore_io("create Restore Coordinator directory", error)),
        };
        validate_directory(&current).await?;
        if created {
            let parent = current.parent().ok_or_else(|| {
                restore_invalid("A Restore Coordinator directory has no owned parent.")
            })?;
            sync_directory(parent).await?;
        }
    }
    Ok(())
}

pub(super) async fn validate_owned_directory_chain(
    state_root: &Path,
    target: &Path,
) -> UseResult<()> {
    if target == state_root || !target.starts_with(state_root) {
        return Err(restore_invalid(
            "A Restore Coordinator directory chain escapes its owned root.",
        ));
    }
    validate_directory(state_root).await?;
    let relative = target.strip_prefix(state_root).map_err(|_| {
        restore_invalid("A Restore Coordinator directory chain is not state-owned.")
    })?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Restore Coordinator directory chain is not normalized.",
        ));
    }
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        validate_directory(&current).await?;
    }
    Ok(())
}

pub(super) async fn validate_staging_entries(
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<()> {
    validate_directory(staging_directory).await?;
    let allowed_directories =
        BTreeSet::from([CANDIDATE_DIRECTORY, RETIRED_DIRECTORY, PUBLISHING_DIRECTORY]);
    let allowed_files = BTreeSet::from([ACTIVATION_FILE, ACTIVATION_PARTIAL_FILE]);
    let mut reader = fs::read_dir(staging_directory)
        .await
        .map_err(|error| restore_io("read Restore Coordinator staging directory", error))?;
    let mut count = 0_usize;
    while let Some(entry) = reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read Restore Coordinator staging entry", error))?
    {
        count += 1;
        if count > allowed_directories.len() + allowed_files.len() {
            return Err(restore_invalid(
                "The Restore Coordinator staging directory contains too many entries.",
            ));
        }
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Restore Coordinator staging names must be valid UTF-8.")
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_io("inspect Restore Coordinator staging entry", error))?;
        let candidate_allowed = !matches!(
            snapshot.manifest.payload,
            ControlRestoreCoordinatorState::Absent
        );
        let valid_directory = allowed_directories.contains(name.as_str())
            && metadata.is_dir()
            && (name != CANDIDATE_DIRECTORY || candidate_allowed);
        let valid_file = allowed_files.contains(name.as_str()) && metadata.is_file();
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || !(valid_directory || valid_file)
        {
            return Err(restore_invalid(
                "The Restore Coordinator staging directory contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

pub(super) async fn require_pre_activation_staging(
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<()> {
    validate_staging_entries(staging_directory, snapshot).await?;
    if optional_regular_file_length(&activation_path(staging_directory))
        .await?
        .is_some()
        || optional_regular_file_length(&activation_partial_path(staging_directory))
            .await?
            .is_some()
        || optional_owned_directory(&retired_path(staging_directory)).await?
        || optional_owned_directory(&publishing_path(staging_directory)).await?
    {
        return Err(restore_invalid(
            "A started Restore Coordinator activation must be reopened under its exclusive guard.",
        ));
    }
    Ok(())
}

pub(super) async fn require_empty_staging(staging_directory: &Path) -> UseResult<()> {
    let mut reader = fs::read_dir(staging_directory)
        .await
        .map_err(|error| restore_io("read absent Restore Coordinator staging", error))?;
    if reader
        .next_entry()
        .await
        .map_err(|error| restore_io("read absent Restore Coordinator entry", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent Restore Coordinator snapshot has unexpected staged state.",
        ));
    }
    Ok(())
}

pub(super) async fn require_candidate_absent(staging_directory: &Path) -> UseResult<()> {
    if optional_owned_directory(&candidate_path(staging_directory)).await? {
        return Err(restore_invalid(
            "An absent Restore Coordinator snapshot has a staged candidate.",
        ));
    }
    Ok(())
}

pub(super) async fn prepare_candidate(
    archive_path: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<CanonicalRestoreHistory> {
    candidate::prepare(archive_path, staging_directory, snapshot).await
}

pub(super) async fn inspect_candidate(
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<CanonicalRestoreHistory> {
    candidate::inspect(staging_directory, snapshot).await
}

pub(super) async fn activate(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<RestoreCoordinatorActivation> {
    validate_owned_directory_chain(state_root, staging_directory).await?;
    reconcile::activate(state_root, staging_directory, snapshot, None).await
}

pub(super) async fn activate_bound(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
    expected: ExpectedActiveRestore<'_>,
) -> UseResult<RestoreCoordinatorActivation> {
    validate_owned_directory_chain(state_root, staging_directory).await?;
    reconcile::activate(state_root, staging_directory, snapshot, Some(expected)).await
}

pub(super) async fn preflight_clean(
    state_root: &Path,
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
) -> UseResult<()> {
    validate_owned_directory_chain(state_root, staging_directory).await?;
    validate_staging_entries(staging_directory, snapshot).await?;
    match snapshot.manifest.payload {
        ControlRestoreCoordinatorState::Absent => {
            require_candidate_absent(staging_directory).await?;
        }
        ControlRestoreCoordinatorState::Archive { .. } => {
            inspect_candidate(staging_directory, snapshot).await?;
        }
    }
    let live = reconcile::scan_live(state_root, snapshot).await?;
    if live.active.is_some() || !live.terminal.is_empty() || live.excluded_active_files != 0 {
        return Err(restore_invalid(
            "The clean Restore Coordinator target contains restore history or active evidence.",
        ));
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
            "A Restore Coordinator path is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(restore_io("inspect Restore Coordinator directory", error)),
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
            "Restore Coordinator evidence is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io("inspect Restore Coordinator evidence", error)),
    }
}

pub(super) async fn read_owned_file(path: &Path, maximum: u64, label: &str) -> UseResult<Vec<u8>> {
    let before = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io(&format!("inspect {label}"), error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&before)
        || !before.is_file()
        || before.len() == 0
        || before.len() > maximum
    {
        return Err(restore_invalid(format!(
            "The {label} is not a bounded owned regular file."
        )));
    }
    let before_modified = before.modified().ok();
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
        .map_err(|error| restore_io(&format!("open {label}"), error))?;
    let capacity = usize::try_from(before.len())
        .map_err(|_| restore_invalid(format!("The {label} length is invalid.")))?;
    let mut bytes = Vec::with_capacity(capacity);
    file.read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_io(&format!("read {label}"), error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| restore_io(&format!("reinspect opened {label}"), error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io(&format!("reinspect {label}"), error))?;
    if bytes.len() as u64 != before.len()
        || a3s_use_core::metadata_is_link_or_reparse_point(&opened)
        || !opened.is_file()
        || opened.len() != before.len()
        || a3s_use_core::metadata_is_link_or_reparse_point(&after)
        || !after.is_file()
        || after.len() != before.len()
        || before_modified.is_some_and(|modified| after.modified().ok() != Some(modified))
    {
        return Err(restore_invalid(format!(
            "The {label} changed while it was read."
        )));
    }
    Ok(bytes)
}

pub(super) async fn publish_noclobber(
    source: PathBuf,
    target: PathBuf,
    action: &'static str,
) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join {action}: {error}")))?
    .map_err(|error| {
        restore_invalid(format!(
            "Failed to {action} '{}': {error}",
            error_target.display()
        ))
    })
}

pub(super) async fn rename_owned(source: &Path, target: &Path, action: &str) -> UseResult<()> {
    match fs::symlink_metadata(target).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Ok(_) => {
            return Err(restore_invalid(format!(
                "The target for {action} already exists."
            )))
        }
        Err(error) => return Err(restore_io(&format!("inspect target for {action}"), error)),
    }
    let worker_source = source.to_path_buf();
    let worker_target = target.to_path_buf();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::rename_path_with_windows_retry_blocking(&worker_source, &worker_target)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join {action}: {error}")))?
    .map_err(|error| restore_io(action, error))?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

pub(super) async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect Restore Coordinator directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Restore Coordinator directory is not an owned directory.",
        ));
    }
    Ok(())
}

pub(super) fn valid_segment(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) fn segment(plan_digest: &str) -> UseResult<&str> {
    plan_digest
        .strip_prefix("sha256:")
        .filter(|value| valid_segment(value))
        .ok_or_else(|| restore_invalid("A Restore Coordinator plan digest is invalid."))
}

pub(super) fn restore_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    restore_invalid(format!("Failed to {action}: {error}"))
}

#[cfg(unix)]
pub(super) async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_io("open Restore Coordinator directory", error))?
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Restore Coordinator directory", error))
}

#[cfg(not(unix))]
pub(super) async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
