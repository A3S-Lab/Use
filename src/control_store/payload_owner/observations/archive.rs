use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseResult};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    observation_error, ControlObservationPayloadEntry, ControlObservationPayloadEntryKind,
    ControlObservationPayloadSnapshot, ControlObservationPayloadState, ControlPayloadOwnerLimits,
};
use crate::cognitive_package::{
    validate_planning_observation_snapshot_record, PlanningObservationSnapshotRecordKind,
};

mod file;
mod live;

use file::{inspect_owned_regular_file, open_owned_regular_file, read_record};
use live::{scan_live, sync_directory, validate_destination};

pub(super) struct CapturedObservationPayload {
    pub(super) payload: ControlObservationPayloadState,
    pub(super) excluded_active_records: u64,
    pub(super) excluded_active_inventory_digest: String,
    pub(super) entries: Vec<ControlObservationPayloadEntry>,
    pub(super) archive_path: Option<PathBuf>,
}

pub(super) async fn snapshot_live(
    state_root: &Path,
    installation: &InstallationId,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<CapturedObservationPayload> {
    validate_destination(state_root, &destination).await?;
    let first = scan_live(state_root, installation, limits).await?;
    if first.terminal.is_empty() {
        return Ok(CapturedObservationPayload {
            payload: ControlObservationPayloadState::Absent,
            excluded_active_records: first.active_count,
            excluded_active_inventory_digest: first.excluded_digest,
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
            .prefix(".a3s-use-control-observations-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| observation_error(format!("Failed to join archive staging: {error}")))?
    .map_err(|error| archive_io("create archive staging", error))?;
    let writer_file = temporary
        .as_file()
        .try_clone()
        .map_err(|error| archive_io("clone archive staging handle", error))?;
    let mut writer = fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    for entry in &first.terminal {
        let bytes = read_record(&entry.source, entry.evidence.length).await?;
        validate_terminal_entry(&entry.evidence, &bytes, installation)?;
        if sha256(&bytes) != entry.evidence.sha256 {
            return Err(observation_error(
                "A terminal observation changed while its archive was written.",
            ));
        }
        writer
            .write_all(&bytes)
            .await
            .map_err(|error| archive_io("write observation archive", error))?;
        archive_digest.update(&bytes);
    }
    writer
        .flush()
        .await
        .map_err(|error| archive_io("flush observation archive", error))?;
    writer
        .sync_all()
        .await
        .map_err(|error| archive_io("synchronize observation archive", error))?;
    drop(writer);

    let second = scan_live(state_root, installation, limits).await?;
    if first != second {
        return Err(observation_error(
            "Planning or diagnostic observations changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| observation_error(format!("Failed to join archive publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            observation_error("The observation snapshot destination already exists.")
        } else {
            archive_io("publish observation archive", error)
        }
    })?;
    sync_directory(&parent).await?;

    let archive_bytes = first.terminal.iter().try_fold(0_u64, |total, entry| {
        total.checked_add(entry.evidence.length)
    });
    let archive_bytes = archive_bytes
        .ok_or_else(|| observation_error("Observation archive byte accounting overflowed."))?;
    Ok(CapturedObservationPayload {
        payload: ControlObservationPayloadState::Archive {
            archive_bytes,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        excluded_active_records: first.active_count,
        excluded_active_inventory_digest: first.excluded_digest,
        entries: first
            .terminal
            .into_iter()
            .map(|entry| entry.evidence)
            .collect(),
        archive_path: Some(destination),
    })
}

pub(super) async fn verify_archive(
    snapshot: &ControlObservationPayloadSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<()> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlObservationPayloadState::Absent, None) if snapshot.manifest.entries.is_empty() => {
            Ok(())
        }
        (
            ControlObservationPayloadState::Archive {
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
        _ => Err(observation_error(
            "Observation archive presence differs from its snapshot manifest.",
        )),
    }
}

async fn verify_archive_file(
    path: &Path,
    expected_bytes: u64,
    expected_digest: &str,
    entries: &[ControlObservationPayloadEntry],
    installation: &InstallationId,
) -> UseResult<()> {
    let (mut reader, before) = open_owned_regular_file(path, "observation archive").await?;
    if before.len() != expected_bytes {
        return Err(observation_error(
            "The observation archive length differs from its manifest.",
        ));
    }
    let before_modified = before.modified().ok();
    let mut archive_digest = Sha256::new();
    let mut package_records = BTreeSet::new();
    for entry in entries {
        let length = usize::try_from(entry.length)
            .map_err(|_| observation_error("An observation record length is invalid."))?;
        let mut bytes = vec![0_u8; length];
        reader
            .read_exact(&mut bytes)
            .await
            .map_err(|_| observation_error("The observation archive is truncated."))?;
        if sha256(&bytes) != entry.sha256 {
            return Err(observation_error(
                "An observation archive record differs from its manifest digest.",
            ));
        }
        let record = validate_terminal_entry(entry, &bytes, installation)?;
        let family = match record.kind {
            PlanningObservationSnapshotRecordKind::DiagnosticHistory => "history",
            PlanningObservationSnapshotRecordKind::TerminalResolution => "resolution",
            PlanningObservationSnapshotRecordKind::ActiveResolution
            | PlanningObservationSnapshotRecordKind::ActiveDownload => {
                return Err(observation_error(
                    "An active planning attempt appeared in a terminal archive.",
                ))
            }
        };
        if !package_records.insert((family, record.package_id)) {
            return Err(observation_error(
                "The observation archive contains duplicate package records.",
            ));
        }
        archive_digest.update(&bytes);
    }
    let mut trailing = [0_u8; 1];
    if reader
        .read(&mut trailing)
        .await
        .map_err(|error| archive_io("finish observation archive", error))?
        != 0
    {
        return Err(observation_error(
            "The observation archive contains trailing unaccounted bytes.",
        ));
    }
    if format!("sha256:{:x}", archive_digest.finalize()) != expected_digest {
        return Err(observation_error(
            "The observation archive digest differs from its manifest.",
        ));
    }
    let opened_after = reader
        .metadata()
        .await
        .map_err(|error| archive_io("reinspect opened observation archive", error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&opened_after)
        || !opened_after.is_file()
        || opened_after.len() != before.len()
        || before_modified.is_some_and(|modified| opened_after.modified().ok() != Some(modified))
    {
        return Err(observation_error(
            "The observation archive changed during offline verification.",
        ));
    }
    let after = inspect_owned_regular_file(path, "observation archive").await?;
    if after.len() != before.len()
        || before_modified.is_some_and(|modified| after.modified().ok() != Some(modified))
    {
        return Err(observation_error(
            "The observation archive changed during offline verification.",
        ));
    }
    Ok(())
}

fn validate_terminal_entry(
    entry: &ControlObservationPayloadEntry,
    bytes: &[u8],
    installation: &InstallationId,
) -> UseResult<crate::cognitive_package::PlanningObservationSnapshotRecord> {
    let record = validate_planning_observation_snapshot_record(&entry.path, bytes, installation)
        .map_err(|_| {
            observation_error("An archived observation failed owner-native validation.")
        })?;
    let expected = match record.kind {
        PlanningObservationSnapshotRecordKind::DiagnosticHistory => {
            ControlObservationPayloadEntryKind::DiagnosticHistory
        }
        PlanningObservationSnapshotRecordKind::TerminalResolution => {
            ControlObservationPayloadEntryKind::TerminalResolution
        }
        PlanningObservationSnapshotRecordKind::ActiveResolution
        | PlanningObservationSnapshotRecordKind::ActiveDownload => {
            return Err(observation_error(
                "An active planning attempt appeared in a terminal archive.",
            ))
        }
    };
    if entry.kind != expected {
        return Err(observation_error(
            "An archived observation kind differs from its decoded record.",
        ));
    }
    Ok(record)
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn archive_io(action: &str, error: io::Error) -> a3s_use_core::UseError {
    observation_error(format!("Failed to {action}: {error}"))
}
