use std::fs::{File, OpenOptions};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use a3s_use_core::{metadata_is_link_or_reparse_point, UseResult};
use sha2::{Digest, Sha256};

use super::{
    snapshot_invalid, snapshot_io, ArchiveEntry, ArchiveEntryKind,
    ControlInstallationSnapshotManifest, ControlPayloadOwnerRegistry,
    MAX_COMPLETE_SNAPSHOT_MANIFEST_BYTES,
};

mod path;

pub(super) use path::{publish, resolve_destination};

const ARCHIVE_MAGIC: &[u8] = b"A3S-USE-CONTROL-SNAPSHOT-V2\n";
const MANIFEST_LENGTH_BYTES: usize = 8;
const MANIFEST_DIGEST_BYTES: usize = 32;
const COPY_BUFFER_BYTES: usize = 128 * 1024;

pub(super) struct ArchiveSources {
    pub(super) control_export: Vec<u8>,
    pub(super) host_projection: PathBuf,
    pub(super) knowledge: PathBuf,
    pub(super) observations: PathBuf,
    pub(super) restore_coordinator: PathBuf,
    pub(super) runtime_plans: PathBuf,
}

pub(super) struct ExtractedArchive {
    pub(super) manifest: ControlInstallationSnapshotManifest,
    pub(super) control_export: Vec<u8>,
    pub(super) host_projection: Option<PathBuf>,
    pub(super) knowledge: Option<PathBuf>,
    pub(super) observations: Option<PathBuf>,
    pub(super) restore_coordinator: Option<PathBuf>,
    pub(super) runtime_plans: Option<PathBuf>,
    pub(super) temporary: tempfile::TempDir,
}

pub(super) fn write_temporary(
    parent: &Path,
    manifest: &ControlInstallationSnapshotManifest,
    sources: &ArchiveSources,
) -> UseResult<tempfile::NamedTempFile> {
    let manifest_bytes = manifest.canonical_bytes()?;
    let entries = manifest.archive_entries()?;
    validate_sources(&entries, sources)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".a3s-use-control-snapshot-")
        .suffix(".tmp")
        .tempfile_in(parent)
        .map_err(|error| snapshot_io(format!("create complete snapshot staging file: {error}")))?;
    {
        let mut writer = BufWriter::new(temporary.as_file_mut());
        writer
            .write_all(ARCHIVE_MAGIC)
            .and_then(|_| writer.write_all(&(manifest_bytes.len() as u64).to_be_bytes()))
            .and_then(|_| writer.write_all(Sha256::digest(&manifest_bytes).as_slice()))
            .and_then(|_| writer.write_all(&manifest_bytes))
            .map_err(|error| {
                snapshot_io(format!("write complete snapshot manifest header: {error}"))
            })?;
        for entry in &entries {
            match entry.kind {
                ArchiveEntryKind::ControlExport => {
                    copy_bytes(&mut writer, &sources.control_export, entry)?;
                }
                kind => copy_file(&mut writer, source_path(sources, kind)?, entry)?,
            }
        }
        writer.flush().map_err(|error| {
            snapshot_io(format!("flush complete snapshot staging file: {error}"))
        })?;
    }
    temporary.as_file().sync_all().map_err(|error| {
        snapshot_io(format!(
            "synchronize complete snapshot staging file: {error}"
        ))
    })?;
    Ok(temporary)
}

pub(super) fn extract(
    registry: &ControlPayloadOwnerRegistry,
    archive_path: &Path,
) -> UseResult<ExtractedArchive> {
    let metadata = std::fs::symlink_metadata(archive_path).map_err(|error| {
        snapshot_invalid(format!(
            "The complete snapshot cannot be inspected: {error}"
        ))
    })?;
    if metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(snapshot_invalid(
            "The complete snapshot is not an owned regular file.",
        ));
    }
    let file = File::open(archive_path).map_err(|error| {
        snapshot_invalid(format!("The complete snapshot cannot be opened: {error}"))
    })?;
    let opened = file.metadata().map_err(|error| {
        snapshot_invalid(format!(
            "The open complete snapshot cannot be inspected: {error}"
        ))
    })?;
    if !opened.is_file() || opened.len() != metadata.len() {
        return Err(snapshot_invalid(
            "The complete snapshot changed while it was opened.",
        ));
    }
    let opened_modified = opened.modified().ok();
    let mut reader = BufReader::new(file);
    let mut magic = vec![0_u8; ARCHIVE_MAGIC.len()];
    read_exact(&mut reader, &mut magic)?;
    if magic != ARCHIVE_MAGIC {
        return Err(snapshot_invalid(
            "The complete snapshot archive magic is invalid.",
        ));
    }
    let mut length_bytes = [0_u8; MANIFEST_LENGTH_BYTES];
    read_exact(&mut reader, &mut length_bytes)?;
    let manifest_length = u64::from_be_bytes(length_bytes);
    if manifest_length == 0 || manifest_length > MAX_COMPLETE_SNAPSHOT_MANIFEST_BYTES as u64 {
        return Err(snapshot_invalid(
            "The complete snapshot manifest length exceeds its bound.",
        ));
    }
    let mut expected_manifest_digest = [0_u8; MANIFEST_DIGEST_BYTES];
    read_exact(&mut reader, &mut expected_manifest_digest)?;
    let manifest_length_usize = usize::try_from(manifest_length)
        .map_err(|_| snapshot_invalid("The complete snapshot manifest length overflowed."))?;
    let mut manifest_bytes = vec![0_u8; manifest_length_usize];
    read_exact(&mut reader, &mut manifest_bytes)?;
    if Sha256::digest(&manifest_bytes).as_slice() != expected_manifest_digest {
        return Err(snapshot_invalid(
            "The complete snapshot manifest digest does not match its header.",
        ));
    }
    let manifest: ControlInstallationSnapshotManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| {
            snapshot_invalid("The complete snapshot manifest is not valid canonical JSON.")
        })?;
    manifest.validate(registry).map_err(|error| {
        snapshot_invalid(format!(
            "The complete snapshot manifest failed validation: {}",
            error.message
        ))
    })?;
    if manifest.canonical_bytes()? != manifest_bytes {
        return Err(snapshot_invalid(
            "The complete snapshot manifest is not canonical JSON.",
        ));
    }
    let entries = manifest.archive_entries()?;
    let expected_length = expected_archive_length(manifest_length, &entries)?;
    if metadata.len() != expected_length {
        return Err(snapshot_invalid(
            "The complete snapshot archive length differs from its manifest.",
        ));
    }

    let temporary = tempfile::Builder::new()
        .prefix("a3s-use-control-snapshot-verify-")
        .tempdir()
        .map_err(|error| {
            snapshot_invalid(format!(
                "The complete snapshot verification directory cannot be created: {error}"
            ))
        })?;
    let mut control_export = None;
    let mut host_projection = None;
    let mut knowledge = None;
    let mut observations = None;
    let mut restore_coordinator = None;
    let mut runtime_plans = None;
    for entry in &entries {
        match entry.kind {
            ArchiveEntryKind::ControlExport => {
                let length = usize::try_from(entry.length).map_err(|_| {
                    snapshot_invalid("The Control export archive length overflowed.")
                })?;
                let mut bytes = vec![0_u8; length];
                read_exact(&mut reader, &mut bytes)?;
                verify_digest(&bytes, entry)?;
                control_export = Some(bytes);
            }
            kind => {
                let path = temporary.path().join(entry_file_name(kind)?);
                extract_file(&mut reader, &path, entry)?;
                match kind {
                    ArchiveEntryKind::HostProjection => host_projection = Some(path),
                    ArchiveEntryKind::Knowledge => knowledge = Some(path),
                    ArchiveEntryKind::Observations => observations = Some(path),
                    ArchiveEntryKind::RestoreCoordinator => restore_coordinator = Some(path),
                    ArchiveEntryKind::RuntimePlans => runtime_plans = Some(path),
                    ArchiveEntryKind::ControlExport => {
                        return Err(snapshot_invalid(
                            "The Control export was classified as an owner payload.",
                        ));
                    }
                }
            }
        }
    }
    let mut trailing = [0_u8; 1];
    if reader
        .read(&mut trailing)
        .map_err(|error| snapshot_invalid(format!("The complete snapshot read failed: {error}")))?
        != 0
    {
        return Err(snapshot_invalid(
            "The complete snapshot contains trailing bytes.",
        ));
    }
    let after = reader.get_ref().metadata().map_err(|error| {
        snapshot_invalid(format!(
            "The verified complete snapshot cannot be inspected: {error}"
        ))
    })?;
    if after.len() != opened.len() || after.modified().ok() != opened_modified {
        return Err(snapshot_invalid(
            "The complete snapshot changed during offline verification.",
        ));
    }
    Ok(ExtractedArchive {
        manifest,
        control_export: control_export
            .ok_or_else(|| snapshot_invalid("The complete snapshot omitted its Control export."))?,
        host_projection,
        knowledge,
        observations,
        restore_coordinator,
        runtime_plans,
        temporary,
    })
}

fn validate_sources(entries: &[ArchiveEntry], sources: &ArchiveSources) -> UseResult<()> {
    let control = entries
        .iter()
        .find(|entry| entry.kind == ArchiveEntryKind::ControlExport)
        .ok_or_else(|| snapshot_invalid("The complete snapshot omitted its Control export."))?;
    if sources.control_export.len() as u64 != control.length
        || digest_bytes(&sources.control_export) != control.sha256
    {
        return Err(snapshot_invalid(
            "The captured Control export differs from the complete snapshot binding.",
        ));
    }
    for kind in [
        ArchiveEntryKind::HostProjection,
        ArchiveEntryKind::Knowledge,
        ArchiveEntryKind::Observations,
        ArchiveEntryKind::RestoreCoordinator,
        ArchiveEntryKind::RuntimePlans,
    ] {
        let path = source_path(sources, kind)?;
        let expected = entries.iter().find(|entry| entry.kind == kind);
        match (expected, std::fs::symlink_metadata(path)) {
            (Some(entry), Ok(metadata))
                if !metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_file()
                    && metadata.len() == entry.length => {}
            (Some(_), _) => {
                return Err(snapshot_invalid(
                    "A complete snapshot payload source is missing, linked, or has drifted.",
                ))
            }
            (None, Err(error)) if error.kind() == io::ErrorKind::NotFound => {}
            (None, _) => {
                return Err(snapshot_invalid(
                    "An absent complete snapshot owner produced unexpected payload bytes.",
                ))
            }
        }
    }
    Ok(())
}

fn source_path(sources: &ArchiveSources, kind: ArchiveEntryKind) -> UseResult<&Path> {
    match kind {
        ArchiveEntryKind::HostProjection => Ok(&sources.host_projection),
        ArchiveEntryKind::Knowledge => Ok(&sources.knowledge),
        ArchiveEntryKind::Observations => Ok(&sources.observations),
        ArchiveEntryKind::RestoreCoordinator => Ok(&sources.restore_coordinator),
        ArchiveEntryKind::RuntimePlans => Ok(&sources.runtime_plans),
        ArchiveEntryKind::ControlExport => Err(snapshot_invalid(
            "The Control export has no external owner payload path.",
        )),
    }
}

fn copy_bytes(writer: &mut impl Write, bytes: &[u8], entry: &ArchiveEntry) -> UseResult<()> {
    if bytes.len() as u64 != entry.length || digest_bytes(bytes) != entry.sha256 {
        return Err(snapshot_invalid(
            "The Control export changed before archive publication.",
        ));
    }
    writer.write_all(bytes).map_err(|error| {
        snapshot_io(format!(
            "write the Control export into the archive: {error}"
        ))
    })
}

fn copy_file(writer: &mut impl Write, path: &Path, entry: &ArchiveEntry) -> UseResult<()> {
    let metadata = std::fs::symlink_metadata(path).map_err(|error| {
        snapshot_invalid(format!(
            "A complete snapshot payload cannot be inspected: {error}"
        ))
    })?;
    if metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != entry.length
    {
        return Err(snapshot_invalid(
            "A complete snapshot payload source is not the exact owned file in its manifest.",
        ));
    }
    let mut source = File::open(path)
        .map_err(|error| snapshot_io(format!("open complete snapshot payload source: {error}")))?;
    let opened = source
        .metadata()
        .map_err(|error| snapshot_io(format!("inspect open complete snapshot payload: {error}")))?;
    if opened.len() != entry.length || !opened.is_file() {
        return Err(snapshot_invalid(
            "A complete snapshot payload changed while it was opened.",
        ));
    }
    let mut digest = Sha256::new();
    let mut remaining = entry.length;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = bounded_copy_length(remaining)?;
        let read = source.read(&mut buffer[..requested]).map_err(|error| {
            snapshot_io(format!("read complete snapshot payload source: {error}"))
        })?;
        if read == 0 {
            return Err(snapshot_invalid(
                "A complete snapshot payload ended before its manifest length.",
            ));
        }
        writer
            .write_all(&buffer[..read])
            .map_err(|error| snapshot_io(format!("write complete snapshot payload: {error}")))?;
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    let mut trailing = [0_u8; 1];
    if source.read(&mut trailing).map_err(|error| {
        snapshot_io(format!("finish reading complete snapshot payload: {error}"))
    })? != 0
        || format!("sha256:{:x}", digest.finalize()) != entry.sha256
    {
        return Err(snapshot_invalid(
            "A complete snapshot payload differs from its owner manifest.",
        ));
    }
    Ok(())
}

fn extract_file(reader: &mut impl Read, path: &Path, entry: &ArchiveEntry) -> UseResult<()> {
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)
        .map_err(|error| {
            snapshot_invalid(format!(
                "A verified complete snapshot payload cannot be created: {error}"
            ))
        })?;
    let mut digest = Sha256::new();
    let mut remaining = entry.length;
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    while remaining > 0 {
        let requested = bounded_copy_length(remaining)?;
        let read = reader.read(&mut buffer[..requested]).map_err(|error| {
            snapshot_invalid(format!(
                "The complete snapshot payload cannot be read: {error}"
            ))
        })?;
        if read == 0 {
            return Err(snapshot_invalid(
                "The complete snapshot payload ended before its manifest length.",
            ));
        }
        output.write_all(&buffer[..read]).map_err(|error| {
            snapshot_invalid(format!(
                "A verified complete snapshot payload cannot be written: {error}"
            ))
        })?;
        digest.update(&buffer[..read]);
        remaining -= read as u64;
    }
    output.flush().map_err(|error| {
        snapshot_invalid(format!(
            "A verified complete snapshot payload cannot be flushed: {error}"
        ))
    })?;
    output.sync_all().map_err(|error| {
        snapshot_invalid(format!(
            "A verified complete snapshot payload cannot be synchronized: {error}"
        ))
    })?;
    if format!("sha256:{:x}", digest.finalize()) != entry.sha256 {
        return Err(snapshot_invalid(
            "A complete snapshot payload digest differs from its owner manifest.",
        ));
    }
    Ok(())
}

fn verify_digest(bytes: &[u8], entry: &ArchiveEntry) -> UseResult<()> {
    if bytes.len() as u64 != entry.length || digest_bytes(bytes) != entry.sha256 {
        return Err(snapshot_invalid(
            "The archived Control export differs from its complete snapshot evidence.",
        ));
    }
    Ok(())
}

fn expected_archive_length(manifest_length: u64, entries: &[ArchiveEntry]) -> UseResult<u64> {
    let header = (ARCHIVE_MAGIC.len() + MANIFEST_LENGTH_BYTES + MANIFEST_DIGEST_BYTES) as u64;
    entries
        .iter()
        .try_fold(
            header.checked_add(manifest_length).ok_or_else(|| {
                snapshot_invalid("The complete snapshot archive length overflowed.")
            })?,
            |total, entry| total.checked_add(entry.length),
        )
        .ok_or_else(|| snapshot_invalid("The complete snapshot archive length overflowed."))
}

fn entry_file_name(kind: ArchiveEntryKind) -> UseResult<&'static str> {
    match kind {
        ArchiveEntryKind::ControlExport => Err(snapshot_invalid(
            "The Control export cannot be extracted as an owner payload.",
        )),
        ArchiveEntryKind::HostProjection => Ok("host-projection.payload"),
        ArchiveEntryKind::Knowledge => Ok("knowledge.sqlite3"),
        ArchiveEntryKind::Observations => Ok("observations.payload"),
        ArchiveEntryKind::RestoreCoordinator => Ok("restore-coordinator.payload"),
        ArchiveEntryKind::RuntimePlans => Ok("runtime-plans.archive"),
    }
}

fn bounded_copy_length(remaining: u64) -> UseResult<usize> {
    usize::try_from(remaining.min(COPY_BUFFER_BYTES as u64))
        .map_err(|_| snapshot_invalid("The complete snapshot copy length overflowed."))
}

fn read_exact(reader: &mut impl Read, bytes: &mut [u8]) -> UseResult<()> {
    reader.read_exact(bytes).map_err(|error| {
        snapshot_invalid(format!(
            "The complete snapshot ended before its declared content: {error}"
        ))
    })
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
