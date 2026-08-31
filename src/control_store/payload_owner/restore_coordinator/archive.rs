use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    coordinator_error, ControlPayloadOwnerLimits, ControlRestoreCoordinatorEntry,
    ControlRestoreCoordinatorSnapshot, ControlRestoreCoordinatorState,
};
use crate::state_restore::{
    read_state_restore_history_snapshot_entry, scan_state_restore_history_snapshot,
    validate_terminal_state_restore_history_record, StateRestoreHistorySnapshotEntry,
    StateRestoreHistorySnapshotScan,
};

pub(super) struct CapturedRestoreCoordinator {
    pub(super) payload: ControlRestoreCoordinatorState,
    pub(super) excluded_active_files: u64,
    pub(super) excluded_active_inventory_digest: String,
    pub(super) entries: Vec<ControlRestoreCoordinatorEntry>,
    pub(super) archive_path: Option<PathBuf>,
}

struct RestoreCoordinatorArchiveReader<'a> {
    reader: fs::File,
    path: PathBuf,
    before_len: u64,
    before_modified: Option<std::time::SystemTime>,
    expected_digest: String,
    entries: &'a [ControlRestoreCoordinatorEntry],
    installation: &'a InstallationId,
    index: usize,
    archive_digest: Sha256,
}

impl<'a> RestoreCoordinatorArchiveReader<'a> {
    async fn open(
        path: &Path,
        expected_bytes: u64,
        expected_digest: &str,
        entries: &'a [ControlRestoreCoordinatorEntry],
        installation: &'a InstallationId,
    ) -> UseResult<Self> {
        let (reader, before) = open_owned_regular_file(path, "Restore Coordinator archive").await?;
        if before.len() != expected_bytes {
            return Err(coordinator_error(
                "The Restore Coordinator archive length differs from its manifest.",
            ));
        }
        Ok(Self {
            reader,
            path: path.to_path_buf(),
            before_len: before.len(),
            before_modified: before.modified().ok(),
            expected_digest: expected_digest.to_owned(),
            entries,
            installation,
            index: 0,
            archive_digest: Sha256::new(),
        })
    }

    async fn next(&mut self) -> UseResult<Option<Vec<u8>>> {
        let Some(entry) = self.entries.get(self.index) else {
            return Ok(None);
        };
        let length = usize::try_from(entry.length)
            .map_err(|_| coordinator_error("A restore history record length is invalid."))?;
        let mut bytes = vec![0_u8; length];
        self.reader
            .read_exact(&mut bytes)
            .await
            .map_err(|_| coordinator_error("The Restore Coordinator archive is truncated."))?;
        if digest(&bytes) != entry.sha256 {
            return Err(coordinator_error(
                "A restore history record differs from its manifest digest.",
            ));
        }
        validate_terminal_state_restore_history_record(
            &entry.plan_digest,
            &bytes,
            self.installation,
        )
        .map_err(wrap_native)?;
        self.archive_digest.update(&bytes);
        self.index += 1;
        Ok(Some(bytes))
    }

    async fn finish(mut self) -> UseResult<()> {
        if self.index != self.entries.len() {
            return Err(coordinator_error(
                "The Restore Coordinator archive was not completely consumed.",
            ));
        }
        let mut trailing = [0_u8; 1];
        if self
            .reader
            .read(&mut trailing)
            .await
            .map_err(|error| archive_io("finish Restore Coordinator archive", error))?
            != 0
        {
            return Err(coordinator_error(
                "The Restore Coordinator archive contains trailing bytes.",
            ));
        }
        if format!("sha256:{:x}", self.archive_digest.finalize()) != self.expected_digest {
            return Err(coordinator_error(
                "The Restore Coordinator archive digest differs from its manifest.",
            ));
        }
        let opened_after =
            self.reader.metadata().await.map_err(|error| {
                archive_io("reinspect opened Restore Coordinator archive", error)
            })?;
        if !owned_regular_metadata(&opened_after)
            || opened_after.len() != self.before_len
            || self
                .before_modified
                .is_some_and(|modified| opened_after.modified().ok() != Some(modified))
        {
            return Err(coordinator_error(
                "The Restore Coordinator archive changed during verification.",
            ));
        }
        let after = inspect_owned_regular_file(&self.path, "Restore Coordinator archive").await?;
        if after.len() != self.before_len
            || self
                .before_modified
                .is_some_and(|modified| after.modified().ok() != Some(modified))
        {
            return Err(coordinator_error(
                "The Restore Coordinator archive changed during verification.",
            ));
        }
        Ok(())
    }
}

pub(super) async fn snapshot_live(
    state_root: &Path,
    installation: &InstallationId,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<CapturedRestoreCoordinator> {
    validate_destination(state_root, &destination).await?;
    let first = scan_state_restore_history_snapshot(state_root, installation)
        .await
        .map_err(wrap_native)?;
    validate_limits(&first, limits)?;
    if first.terminal.is_empty() {
        let second = scan_state_restore_history_snapshot(state_root, installation)
            .await
            .map_err(wrap_native)?;
        if first != second {
            return Err(coordinator_error(
                "Restore Coordinator history changed during snapshot creation.",
            ));
        }
        return Ok(CapturedRestoreCoordinator {
            payload: ControlRestoreCoordinatorState::Absent,
            excluded_active_files: first.excluded_active_files,
            excluded_active_inventory_digest: first.excluded_active_inventory_digest,
            entries: Vec::new(),
            archive_path: None,
        });
    }

    let parent = destination
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_path_buf();
    let temporary_parent = parent.clone();
    let temporary = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".a3s-use-control-restore-coordinator-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| coordinator_error(format!("Failed to join archive staging: {error}")))?
    .map_err(|error| archive_io("create Restore Coordinator archive staging", error))?;
    let writer_file = temporary
        .as_file()
        .try_clone()
        .map_err(|error| archive_io("clone Restore Coordinator staging handle", error))?;
    let mut writer = fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    for entry in &first.terminal {
        let bytes = read_state_restore_history_snapshot_entry(entry, installation)
            .await
            .map_err(wrap_native)?;
        writer
            .write_all(&bytes)
            .await
            .map_err(|error| archive_io("write Restore Coordinator archive", error))?;
        archive_digest.update(&bytes);
    }
    writer
        .flush()
        .await
        .map_err(|error| archive_io("flush Restore Coordinator archive", error))?;
    writer
        .sync_all()
        .await
        .map_err(|error| archive_io("synchronize Restore Coordinator archive", error))?;
    drop(writer);

    let second = scan_state_restore_history_snapshot(state_root, installation)
        .await
        .map_err(wrap_native)?;
    if first != second {
        return Err(coordinator_error(
            "Restore Coordinator history changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| coordinator_error(format!("Failed to join archive publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            coordinator_error("The Restore Coordinator snapshot destination already exists.")
        } else {
            archive_io("publish Restore Coordinator archive", error)
        }
    })?;
    sync_directory(&parent).await?;

    let archive_bytes = first
        .terminal
        .iter()
        .try_fold(0_u64, |total, entry| total.checked_add(entry.length));
    let archive_bytes = archive_bytes
        .ok_or_else(|| coordinator_error("Restore Coordinator byte accounting overflowed."))?;
    Ok(CapturedRestoreCoordinator {
        payload: ControlRestoreCoordinatorState::Archive {
            archive_bytes,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        excluded_active_files: first.excluded_active_files,
        excluded_active_inventory_digest: first.excluded_active_inventory_digest,
        entries: first
            .terminal
            .into_iter()
            .map(
                |entry: StateRestoreHistorySnapshotEntry| ControlRestoreCoordinatorEntry {
                    plan_digest: entry.plan_digest,
                    length: entry.length,
                    sha256: entry.sha256,
                },
            )
            .collect(),
        archive_path: Some(destination),
    })
}

pub(super) async fn verify_archive(
    snapshot: &ControlRestoreCoordinatorSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<()> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlRestoreCoordinatorState::Absent, None) if snapshot.manifest.entries.is_empty() => {
            Ok(())
        }
        (
            ControlRestoreCoordinatorState::Archive {
                archive_bytes,
                archive_sha256,
            },
            Some(path),
        ) => {
            let mut reader = RestoreCoordinatorArchiveReader::open(
                path,
                *archive_bytes,
                archive_sha256,
                &snapshot.manifest.entries,
                &snapshot.manifest.binding.installation,
            )
            .await?;
            while reader.next().await?.is_some() {}
            reader.finish().await
        }
        _ => Err(coordinator_error(
            "Restore Coordinator archive presence differs from its manifest.",
        )),
    }
}

fn validate_limits(
    scan: &StateRestoreHistorySnapshotScan,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<()> {
    let terminal_files = u64::try_from(scan.terminal.len())
        .map_err(|_| coordinator_error("Restore Coordinator file accounting overflowed."))?;
    let scanned_files = terminal_files
        .checked_add(scan.excluded_active_files)
        .ok_or_else(|| coordinator_error("Restore Coordinator file accounting overflowed."))?;
    let bytes = scan
        .terminal
        .iter()
        .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
        .ok_or_else(|| coordinator_error("Restore Coordinator byte accounting overflowed."))?;
    if scanned_files > limits.max_files || bytes > limits.max_payload_bytes {
        return Err(coordinator_error(
            "Restore Coordinator history exceeds its registered bounds.",
        ));
    }
    Ok(())
}

async fn validate_destination(state_root: &Path, destination: &Path) -> UseResult<()> {
    let file_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| coordinator_error("The Restore Coordinator archive must name a file."))?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    owned_directory(parent).await?;
    let physical_parent = fs::canonicalize(parent)
        .await
        .map_err(|error| archive_io("resolve Restore Coordinator archive parent", error))?;
    let physical_state = fs::canonicalize(state_root)
        .await
        .map_err(|error| archive_io("resolve Restore Coordinator state root", error))?;
    if physical_parent.join(file_name).starts_with(physical_state) {
        return Err(coordinator_error(
            "The Restore Coordinator archive must remain outside Use-owned state.",
        ));
    }
    match fs::symlink_metadata(destination).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(coordinator_error(
            "The Restore Coordinator snapshot destination already exists.",
        )),
        Err(error) => Err(archive_io(
            "inspect Restore Coordinator archive destination",
            error,
        )),
    }
}

async fn open_owned_regular_file(
    path: &Path,
    label: &str,
) -> UseResult<(fs::File, std::fs::Metadata)> {
    inspect_owned_regular_file(path, label).await?;
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
    let file = options
        .open(path)
        .await
        .map_err(|error| archive_io(&format!("open {label}"), error))?;
    let metadata = file
        .metadata()
        .await
        .map_err(|error| archive_io(&format!("inspect opened {label}"), error))?;
    if !owned_regular_metadata(&metadata) {
        return Err(coordinator_error(format!(
            "The {label} is not an owned regular file."
        )));
    }
    Ok((file, metadata))
}

async fn inspect_owned_regular_file(path: &Path, label: &str) -> UseResult<std::fs::Metadata> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| archive_io(&format!("inspect {label}"), error))?;
    if !owned_regular_metadata(&metadata) {
        return Err(coordinator_error(format!(
            "The {label} is not an owned regular file."
        )));
    }
    Ok(metadata)
}

fn owned_regular_metadata(metadata: &std::fs::Metadata) -> bool {
    !a3s_use_core::metadata_is_link_or_reparse_point(metadata) && metadata.is_file()
}

async fn owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| archive_io("inspect Restore Coordinator directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(coordinator_error(
            "A Restore Coordinator directory is not owned.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| archive_io("open Restore Coordinator archive directory", error))?
        .sync_all()
        .await
        .map_err(|error| archive_io("sync Restore Coordinator archive directory", error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn wrap_native(error: a3s_use_core::UseError) -> a3s_use_core::UseError {
    coordinator_error(format!(
        "Restore Coordinator owner validation failed: {}",
        error.message
    ))
}

fn archive_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    coordinator_error(format!("Failed to {action}: {error}"))
}
