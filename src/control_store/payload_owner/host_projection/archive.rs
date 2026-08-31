use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    host_projection_error, ControlHostProjectionEntry, ControlHostProjectionEntryKind,
    ControlHostProjectionSnapshot, ControlHostProjectionState, ControlPayloadOwnerLimits,
};
use crate::cognitive_package::{
    scan_host_projection_snapshot, validate_host_projection_snapshot_record,
    validate_host_projection_snapshot_set, HostProjectionSnapshotRecord,
    HostProjectionSnapshotRecordKind,
};

mod file;

use file::read_record;
pub(super) use file::{inspect_owned_regular_file, open_owned_regular_file};

pub(super) struct CapturedHostProjection {
    pub(super) payload: ControlHostProjectionState,
    pub(super) validated_index_records: u64,
    pub(super) entries: Vec<ControlHostProjectionEntry>,
    pub(super) archive_path: Option<PathBuf>,
}

pub(super) struct HostProjectionArchiveReader<'a> {
    reader: fs::File,
    path: PathBuf,
    before_len: u64,
    before_modified: Option<std::time::SystemTime>,
    expected_digest: String,
    entries: &'a [ControlHostProjectionEntry],
    installation: &'a InstallationId,
    index: usize,
    archive_digest: Sha256,
}

impl<'a> HostProjectionArchiveReader<'a> {
    pub(super) async fn open(
        path: &Path,
        expected_bytes: u64,
        expected_digest: &str,
        entries: &'a [ControlHostProjectionEntry],
        installation: &'a InstallationId,
    ) -> UseResult<Self> {
        let (reader, before) = open_owned_regular_file(path, "Host projection archive").await?;
        if before.len() != expected_bytes {
            return Err(host_projection_error(
                "The Host projection archive length differs from its manifest.",
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

    pub(super) async fn next(
        &mut self,
    ) -> UseResult<
        Option<(
            ControlHostProjectionEntry,
            Vec<u8>,
            HostProjectionSnapshotRecord,
        )>,
    > {
        let Some(entry) = self.entries.get(self.index).cloned() else {
            return Ok(None);
        };
        let length = usize::try_from(entry.length)
            .map_err(|_| host_projection_error("A Host archive record length is invalid."))?;
        let mut bytes = vec![0_u8; length];
        self.reader
            .read_exact(&mut bytes)
            .await
            .map_err(|_| host_projection_error("The Host projection archive is truncated."))?;
        if sha256(&bytes) != entry.sha256 {
            return Err(host_projection_error(
                "A Host archive record differs from its manifest digest.",
            ));
        }
        let record =
            validate_host_projection_snapshot_record(&entry.path, &bytes, self.installation)
                .map_err(|_| {
                    host_projection_error("An archived Host record failed owner-native validation.")
                })?;
        let expected_kind = match record.kind() {
            HostProjectionSnapshotRecordKind::Request => ControlHostProjectionEntryKind::Request,
            HostProjectionSnapshotRecordKind::Cancellation => {
                ControlHostProjectionEntryKind::Cancellation
            }
        };
        if entry.kind != expected_kind {
            return Err(host_projection_error(
                "An archived Host record kind differs from its manifest.",
            ));
        }
        self.archive_digest.update(&bytes);
        self.index += 1;
        Ok(Some((entry, bytes, record)))
    }

    pub(super) async fn finish(mut self) -> UseResult<()> {
        if self.index != self.entries.len() {
            return Err(host_projection_error(
                "The Host projection archive was not completely consumed.",
            ));
        }
        let mut trailing = [0_u8; 1];
        if self
            .reader
            .read(&mut trailing)
            .await
            .map_err(|error| archive_io("finish Host projection archive", error))?
            != 0
        {
            return Err(host_projection_error(
                "The Host projection archive contains trailing unaccounted bytes.",
            ));
        }
        if format!("sha256:{:x}", self.archive_digest.finalize()) != self.expected_digest {
            return Err(host_projection_error(
                "The Host projection archive digest differs from its manifest.",
            ));
        }
        let opened_after = self
            .reader
            .metadata()
            .await
            .map_err(|error| archive_io("reinspect opened Host projection archive", error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&opened_after)
            || !opened_after.is_file()
            || opened_after.len() != self.before_len
            || self
                .before_modified
                .is_some_and(|modified| opened_after.modified().ok() != Some(modified))
        {
            return Err(host_projection_error(
                "The Host projection archive changed during offline verification.",
            ));
        }
        let after = inspect_owned_regular_file(&self.path, "Host projection archive").await?;
        if after.len() != self.before_len
            || self
                .before_modified
                .is_some_and(|modified| after.modified().ok() != Some(modified))
        {
            return Err(host_projection_error(
                "The Host projection archive changed during offline verification.",
            ));
        }
        Ok(())
    }
}

pub(super) async fn snapshot_live<F>(
    state_root: &Path,
    installation: &InstallationId,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
    validate_control: F,
) -> UseResult<CapturedHostProjection>
where
    F: Fn(&[HostProjectionSnapshotRecord]) -> UseResult<()>,
{
    validate_destination(state_root, &destination).await?;
    let first = scan_host_projection_snapshot(
        state_root,
        installation,
        limits.max_files,
        limits.max_payload_bytes,
    )
    .await?;
    let entries = first
        .sources
        .iter()
        .map(|source| ControlHostProjectionEntry {
            kind: source.kind.into(),
            path: source.logical_path.clone(),
            length: source.length,
            sha256: source.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let records = first
        .sources
        .iter()
        .map(|source| source.record.clone())
        .collect::<Vec<_>>();
    validate_control(&records)?;
    if first.sources.is_empty() {
        return Ok(CapturedHostProjection {
            payload: ControlHostProjectionState::Absent,
            validated_index_records: first.validated_index_records,
            entries,
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
            .prefix(".a3s-use-control-host-projection-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| host_projection_error(format!("Failed to join archive staging: {error}")))?
    .map_err(|error| archive_io("create Host archive staging", error))?;
    let writer_file = temporary
        .as_file()
        .try_clone()
        .map_err(|error| archive_io("clone Host archive staging handle", error))?;
    let mut writer = fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    for source in &first.sources {
        let bytes = read_record(&source.source, source.length).await?;
        let record =
            validate_host_projection_snapshot_record(&source.logical_path, &bytes, installation)?;
        if record != source.record || sha256(&bytes) != source.sha256 {
            return Err(host_projection_error(
                "A Host semantic record changed while its archive was written.",
            ));
        }
        writer
            .write_all(&bytes)
            .await
            .map_err(|error| archive_io("write Host projection archive", error))?;
        archive_digest.update(&bytes);
    }
    writer
        .flush()
        .await
        .map_err(|error| archive_io("flush Host projection archive", error))?;
    writer
        .sync_all()
        .await
        .map_err(|error| archive_io("synchronize Host projection archive", error))?;
    drop(writer);

    let second = scan_host_projection_snapshot(
        state_root,
        installation,
        limits.max_files,
        limits.max_payload_bytes,
    )
    .await?;
    if first != second {
        return Err(host_projection_error(
            "The Host protocol projection changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| host_projection_error(format!("Failed to join archive publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            host_projection_error("The Host projection snapshot destination already exists.")
        } else {
            archive_io("publish Host projection archive", error)
        }
    })?;
    sync_directory(&parent).await?;

    let archive_bytes = entries
        .iter()
        .try_fold(0_u64, |total, entry| total.checked_add(entry.length));
    let archive_bytes = archive_bytes
        .ok_or_else(|| host_projection_error("Host archive byte accounting overflowed."))?;
    Ok(CapturedHostProjection {
        payload: ControlHostProjectionState::Archive {
            archive_bytes,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        validated_index_records: first.validated_index_records,
        entries,
        archive_path: Some(destination),
    })
}

pub(super) async fn verify_archive(
    snapshot: &ControlHostProjectionSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<Vec<HostProjectionSnapshotRecord>> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlHostProjectionState::Absent, None) if snapshot.manifest.entries.is_empty() => {
            let records = Vec::new();
            validate_host_projection_snapshot_set(
                &records,
                &snapshot.manifest.binding.installation,
            )?;
            Ok(records)
        }
        (
            ControlHostProjectionState::Archive {
                archive_bytes,
                archive_sha256,
            },
            Some(path),
        ) => {
            verify_archive_file(
                path,
                *archive_bytes,
                archive_sha256,
                &snapshot.manifest.entries,
                &snapshot.manifest.binding.installation,
            )
            .await
        }
        _ => Err(host_projection_error(
            "Host archive presence differs from its snapshot manifest.",
        )),
    }
}

async fn verify_archive_file(
    path: &Path,
    expected_bytes: u64,
    expected_digest: &str,
    entries: &[ControlHostProjectionEntry],
    installation: &InstallationId,
) -> UseResult<Vec<HostProjectionSnapshotRecord>> {
    let mut reader = HostProjectionArchiveReader::open(
        path,
        expected_bytes,
        expected_digest,
        entries,
        installation,
    )
    .await?;
    let mut records = Vec::with_capacity(entries.len());
    while let Some((_, _, record)) = reader.next().await? {
        records.push(record);
    }
    reader.finish().await?;
    validate_host_projection_snapshot_set(&records, installation)?;
    Ok(records)
}

async fn validate_destination(state_root: &Path, destination: &Path) -> UseResult<()> {
    let file_name = destination
        .file_name()
        .filter(|name| !name.is_empty())
        .ok_or_else(|| host_projection_error("The Host archive must name a file."))?;
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    owned_directory(parent).await?;
    let physical_parent = fs::canonicalize(parent)
        .await
        .map_err(|error| archive_io("resolve Host archive parent", error))?;
    let physical_state = fs::canonicalize(state_root)
        .await
        .map_err(|error| archive_io("resolve Host state root", error))?;
    if physical_parent.join(file_name).starts_with(&physical_state) {
        return Err(host_projection_error(
            "The Host archive destination must remain outside Use-owned state.",
        ));
    }
    match fs::symlink_metadata(destination).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(host_projection_error(
            "The Host projection snapshot destination already exists.",
        )),
        Err(error) => Err(archive_io("inspect Host archive destination", error)),
    }
}

async fn owned_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| archive_io("inspect Host archive directory", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(host_projection_error(
            "A Host archive directory is not an owned directory.",
        ));
    }
    Ok(())
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

pub(super) fn archive_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    host_projection_error(format!("Failed to {action}: {error}"))
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| archive_io("open Host archive directory", error))?
        .sync_all()
        .await
        .map_err(|error| archive_io("sync Host archive directory", error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
