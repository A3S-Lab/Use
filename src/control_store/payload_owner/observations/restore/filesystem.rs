use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{restore_invalid, restore_target_not_empty};
use crate::control_store::payload_owner::observations::{
    archive, ControlObservationPayloadEntry, ControlObservationPayloadSnapshot,
    ControlObservationPayloadState,
};

const CANDIDATE_FILE: &str = "control-observations.archive";
const ARCHIVE_PARTIAL_FILE: &str = "control-observations.archive.partial";
const ACTIVATING_FILE: &str = "control-observations.archive.activating";

mod publication;

pub(super) use publication::activate_archive;

pub(super) enum StagedArchiveState {
    Ready(PathBuf),
    Activating(PathBuf),
}

impl StagedArchiveState {
    pub(super) fn path(&self) -> &Path {
        match self {
            Self::Ready(path) | Self::Activating(path) => path,
        }
    }
}

pub(super) fn candidate_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(CANDIDATE_FILE)
}

pub(super) fn validate_staging_location(
    state_root: &Path,
    staging_directory: &Path,
) -> UseResult<()> {
    if staging_directory == state_root || !staging_directory.starts_with(state_root) {
        return Err(restore_invalid(
            "The observation restore staging directory escapes the target state root.",
        ));
    }
    let relative = staging_directory
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("The observation restore staging path is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "The observation restore staging directory is not a normalized owned path.",
        ));
    }
    let operations = state_root.join("operations");
    if [
        "package-diagnostic-history",
        "package-downloads",
        "package-resolutions",
    ]
    .into_iter()
    .any(|owner| staging_directory.starts_with(operations.join(owner)))
    {
        return Err(restore_invalid(
            "The observation restore candidate cannot be staged inside a live owner root.",
        ));
    }
    Ok(())
}

pub(super) async fn stage_archive(
    source: &Path,
    staging_directory: &Path,
    snapshot: &ControlObservationPayloadSnapshot,
) -> UseResult<()> {
    validate_staging_entries(staging_directory, snapshot).await?;
    let candidate = candidate_path(staging_directory);
    let partial = staging_directory.join(ARCHIVE_PARTIAL_FILE);
    let activating = staging_directory.join(ACTIVATING_FILE);
    let candidate_exists = optional_regular_file(&candidate).await?;
    let partial_exists = optional_regular_file(&partial).await?;
    let activating_exists = optional_regular_file(&activating).await?;
    if [candidate_exists, partial_exists, activating_exists]
        .into_iter()
        .filter(|present| *present)
        .count()
        > 1
    {
        return Err(restore_invalid(
            "The observation restore staging directory has ambiguous archive state.",
        ));
    }
    if activating_exists {
        verify_archive(snapshot, &activating).await?;
        return Ok(());
    }
    if any_record_partial(staging_directory, snapshot).await? {
        return Err(restore_invalid(
            "A staged observation record exists before activation has started.",
        ));
    }
    if candidate_exists {
        verify_archive(snapshot, &candidate).await?;
        return Ok(());
    }

    let expected_bytes = archive_bytes(snapshot)?;
    if partial_exists {
        let bytes = archive::inspect_owned_regular_file(&partial, "observation restore partial")
            .await
            .map_err(wrap_archive_error)?
            .len();
        if bytes == expected_bytes && verify_archive(snapshot, &partial).await.is_ok() {
            publish_noclobber(
                partial,
                candidate.clone(),
                "publish observation restore archive",
                false,
            )
            .await?;
            sync_directory(staging_directory).await?;
            return Ok(());
        }
        if bytes >= expected_bytes {
            return Err(restore_invalid(
                "The partial observation restore archive has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete observation archive", error))?;
        sync_directory(staging_directory).await?;
    }

    verify_archive(snapshot, source).await?;
    let (mut input, metadata) =
        archive::open_owned_regular_file(source, "observation restore source")
            .await
            .map_err(wrap_archive_error)?;
    if metadata.len() != expected_bytes {
        return Err(restore_invalid(
            "The verified observation restore source changed before staging.",
        ));
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create partial observation restore archive", error))?;
    let copied = tokio::io::copy(
        &mut (&mut input).take(expected_bytes.saturating_add(1)),
        &mut output,
    )
    .await
    .map_err(|error| restore_io("copy observation restore archive", error))?;
    if copied != expected_bytes {
        return Err(restore_invalid(
            "The verified observation restore source changed while it was staged.",
        ));
    }
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush observation restore archive", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync observation restore archive", error))?;
    drop(output);
    verify_archive(snapshot, &partial).await?;
    publish_noclobber(
        partial,
        candidate,
        "publish observation restore archive",
        false,
    )
    .await?;
    sync_directory(staging_directory).await
}

pub(super) async fn staged_archive_state(
    staging_directory: &Path,
    snapshot: &ControlObservationPayloadSnapshot,
) -> UseResult<StagedArchiveState> {
    validate_staging_entries(staging_directory, snapshot).await?;
    let candidate = candidate_path(staging_directory);
    let activating = staging_directory.join(ACTIVATING_FILE);
    let archive_partial = staging_directory.join(ARCHIVE_PARTIAL_FILE);
    let candidate_exists = optional_regular_file(&candidate).await?;
    let activating_exists = optional_regular_file(&activating).await?;
    if optional_regular_file(&archive_partial).await?
        || candidate_exists == activating_exists
        || candidate_exists && any_record_partial(staging_directory, snapshot).await?
    {
        return Err(restore_invalid(
            "The observation restore staging state is incomplete or ambiguous.",
        ));
    }
    Ok(if candidate_exists {
        StagedArchiveState::Ready(candidate)
    } else {
        StagedArchiveState::Activating(activating)
    })
}

pub(super) async fn begin_activation(
    candidate: PathBuf,
    staging_directory: &Path,
) -> UseResult<PathBuf> {
    if candidate != candidate_path(staging_directory) {
        return Err(restore_invalid(
            "The observation restore candidate is outside its exact staging path.",
        ));
    }
    let activating = staging_directory.join(ACTIVATING_FILE);
    publish_noclobber(
        candidate,
        activating.clone(),
        "mark observation restore activation",
        false,
    )
    .await?;
    sync_directory(staging_directory).await?;
    Ok(activating)
}

pub(super) async fn require_empty_staging(staging_directory: &Path) -> UseResult<()> {
    let mut entries = fs::read_dir(staging_directory)
        .await
        .map_err(|error| restore_io("read observation restore staging directory", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read observation restore staging entry", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent observation snapshot has unexpected staged bytes.",
        ));
    }
    Ok(())
}

pub(super) async fn validate_staging_entries(
    staging_directory: &Path,
    snapshot: &ControlObservationPayloadSnapshot,
) -> UseResult<()> {
    validate_directory(staging_directory).await?;
    let mut allowed = BTreeSet::from([
        CANDIDATE_FILE.to_owned(),
        ARCHIVE_PARTIAL_FILE.to_owned(),
        ACTIVATING_FILE.to_owned(),
    ]);
    for (index, entry) in snapshot.manifest.entries.iter().enumerate() {
        allowed.insert(record_partial_name(index, entry));
    }
    let mut entries = fs::read_dir(staging_directory)
        .await
        .map_err(|error| restore_io("read observation restore staging directory", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_io("read observation restore staging entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Observation restore staging names must be valid UTF-8.")
        })?;
        if !allowed.contains(&name) {
            return Err(restore_invalid(
                "The observation restore staging directory contains an unowned entry.",
            ));
        }
        optional_regular_file(&entry.path()).await?;
    }
    Ok(())
}

pub(super) async fn ensure_owned_directory(state_root: &Path, target: &Path) -> UseResult<()> {
    if target == state_root || !target.starts_with(state_root) {
        return Err(restore_invalid(
            "An observation restore directory escapes the target state root.",
        ));
    }
    validate_directory(state_root).await?;
    let relative = target
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("An observation restore directory is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "An observation restore directory is not a normalized owned path.",
        ));
    }
    let mut current = state_root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(restore_io("create observation restore directory", error)),
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

fn record_partial_path(
    staging_directory: &Path,
    index: usize,
    entry: &ControlObservationPayloadEntry,
) -> PathBuf {
    staging_directory.join(record_partial_name(index, entry))
}

fn record_partial_name(index: usize, entry: &ControlObservationPayloadEntry) -> String {
    let digest = entry.sha256.strip_prefix("sha256:").unwrap_or("invalid");
    format!("record-{index:010}-{digest}.partial")
}

async fn any_record_partial(
    staging_directory: &Path,
    snapshot: &ControlObservationPayloadSnapshot,
) -> UseResult<bool> {
    for (index, entry) in snapshot.manifest.entries.iter().enumerate() {
        if optional_regular_file(&record_partial_path(staging_directory, index, entry)).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

fn archive_bytes(snapshot: &ControlObservationPayloadSnapshot) -> UseResult<u64> {
    archive_evidence(snapshot).map(|(bytes, _)| bytes)
}

fn archive_evidence(snapshot: &ControlObservationPayloadSnapshot) -> UseResult<(u64, &str)> {
    match &snapshot.manifest.payload {
        ControlObservationPayloadState::Archive {
            archive_bytes,
            archive_sha256,
        } => Ok((*archive_bytes, archive_sha256)),
        ControlObservationPayloadState::Absent => Err(restore_invalid(
            "An absent observation snapshot has no archive evidence.",
        )),
    }
}

async fn verify_archive(
    snapshot: &ControlObservationPayloadSnapshot,
    path: &Path,
) -> UseResult<()> {
    archive::verify_archive(snapshot, Some(path))
        .await
        .map_err(wrap_archive_error)
}

async fn publish_noclobber(
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

async fn optional_regular_file(path: &Path) -> UseResult<bool> {
    Ok(optional_regular_file_length(path).await?.is_some())
}

async fn optional_regular_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "An observation restore file is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io("inspect observation restore file", error)),
    }
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect observation restore directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "An observation restore directory is not an owned directory.",
        ));
    }
    Ok(())
}

fn wrap_archive_error(error: a3s_use_core::UseError) -> a3s_use_core::UseError {
    restore_invalid(format!(
        "Observation restore archive verification failed: {}",
        error.message
    ))
}

fn restore_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    restore_invalid(format!("Failed to {action}: {error}"))
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| restore_io("open observation restore directory", error))?
        .sync_all()
        .await
        .map_err(|error| restore_io("sync observation restore directory", error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
