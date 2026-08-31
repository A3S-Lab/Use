use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{
    candidate_path, ensure_owned_directory, optional_owned_directory, optional_regular_file_length,
    publish_noclobber, read_exact_owned, restore_io, sync_directory, wrap_archive_error,
};
use crate::cognitive_package::{
    scan_host_projection_snapshot, HostProjectionRestoreIndexBuilder, HostProjectionSnapshotRecord,
};
use crate::control_store::payload_owner::host_projection::{
    archive, restore::restore_invalid, ControlHostProjectionEntry, ControlHostProjectionSnapshot,
    ControlHostProjectionState, ControlPayloadOwnerLimits,
};

#[derive(Debug, Clone, PartialEq, Eq)]
struct CanonicalHostProjectionFile {
    logical_path: String,
    length: u64,
    sha256: String,
}

#[derive(Debug)]
pub(in crate::control_store::payload_owner::host_projection::restore) struct CanonicalHostProjection
{
    files: Vec<CanonicalHostProjectionFile>,
    normalized_index_records: u64,
}

pub(super) async fn prepare(
    archive_path: &Path,
    staging_directory: &Path,
    snapshot: &ControlHostProjectionSnapshot,
    expected_records: &[HostProjectionSnapshotRecord],
    limits: ControlPayloadOwnerLimits,
    build_if_missing: bool,
) -> UseResult<CanonicalHostProjection> {
    let candidate = candidate_path(staging_directory);
    let existed = optional_owned_directory(&candidate).await?;
    if build_if_missing && !existed {
        ensure_owned_directory(staging_directory, &candidate).await?;
    }
    let build = build_if_missing;
    let (archive_bytes, archive_sha256) = match &snapshot.manifest.payload {
        ControlHostProjectionState::Archive {
            archive_bytes,
            archive_sha256,
        } => (*archive_bytes, archive_sha256.as_str()),
        ControlHostProjectionState::Absent => {
            return Err(restore_invalid(
                "An absent Host projection cannot build a restore candidate.",
            ))
        }
    };
    let mut reader = archive::HostProjectionArchiveReader::open(
        archive_path,
        archive_bytes,
        archive_sha256,
        &snapshot.manifest.entries,
        &snapshot.manifest.binding.installation,
    )
    .await
    .map_err(wrap_archive_error)?;
    let mut indexes =
        HostProjectionRestoreIndexBuilder::new(snapshot.manifest.binding.installation.clone())
            .map_err(wrap_archive_error)?;
    let mut source_files = Vec::with_capacity(snapshot.manifest.entries.len());
    let mut observed = 0_usize;
    while let Some((entry, bytes, record)) = reader.next().await.map_err(wrap_archive_error)? {
        let expected = expected_records.get(observed).ok_or_else(|| {
            restore_invalid("The verified Host record sequence is shorter than its archive.")
        })?;
        let derived_record = indexes
            .observe(&entry.path, &bytes)
            .map_err(wrap_archive_error)?;
        if &record != expected || derived_record != record {
            return Err(restore_invalid(
                "The staged Host archive differs from its offline-verified record sequence.",
            ));
        }
        if build {
            write_candidate_file(&candidate, &entry.path, &bytes, &entry.sha256).await?;
        }
        source_files.push(CanonicalHostProjectionFile {
            logical_path: entry.path,
            length: entry.length,
            sha256: entry.sha256,
        });
        observed += 1;
    }
    reader.finish().await.map_err(wrap_archive_error)?;
    if observed != expected_records.len() {
        return Err(restore_invalid(
            "The verified Host record sequence is longer than its archive.",
        ));
    }

    let derived = indexes.finish().map_err(wrap_archive_error)?;
    let normalized_index_records = u64::try_from(derived.len())
        .map_err(|_| restore_invalid("The normalized Host index count overflowed."))?;
    if normalized_index_records > snapshot.manifest.validated_index_records {
        return Err(restore_invalid(
            "The canonical Host indexes exceed the snapshot's validated index inventory.",
        ));
    }
    let mut derived_files = Vec::with_capacity(derived.len());
    for file in derived {
        let digest = sha256(&file.bytes);
        if build {
            write_candidate_file(&candidate, &file.logical_path, &file.bytes, &digest).await?;
        }
        derived_files.push(CanonicalHostProjectionFile {
            logical_path: file.logical_path,
            length: file.bytes.len() as u64,
            sha256: digest,
        });
    }
    source_files.extend(derived_files);
    let canonical = CanonicalHostProjection::new(source_files, normalized_index_records, limits)?;
    if optional_owned_directory(&candidate).await? {
        validate_projection_root(
            staging_directory,
            snapshot,
            expected_records,
            &canonical,
            limits,
        )
        .await?;
    } else if build {
        return Err(restore_invalid(
            "The canonical Host restore candidate disappeared while it was built.",
        ));
    }
    Ok(canonical)
}

impl CanonicalHostProjection {
    fn new(
        mut files: Vec<CanonicalHostProjectionFile>,
        normalized_index_records: u64,
        limits: ControlPayloadOwnerLimits,
    ) -> UseResult<Self> {
        files.sort_by(|left, right| left.logical_path.cmp(&right.logical_path));
        let mut portable = BTreeSet::new();
        let mut bytes = 0_u64;
        for file in &files {
            if file.logical_path.is_empty()
                || file.length == 0
                || !crate::control_store::model::valid_sha256(&file.sha256)
                || !portable.insert(file.logical_path.to_ascii_lowercase())
            {
                return Err(restore_invalid(
                    "The canonical Host restore file inventory is invalid or nonportable.",
                ));
            }
            bytes = bytes.checked_add(file.length).ok_or_else(|| {
                restore_invalid("The canonical Host restore byte count overflowed.")
            })?;
        }
        let count = u64::try_from(files.len())
            .map_err(|_| restore_invalid("The canonical Host restore file count overflowed."))?;
        if count > limits.max_files || bytes > limits.max_payload_bytes {
            return Err(restore_invalid(
                "The canonical Host restore candidate exceeds its owner bounds.",
            ));
        }
        Ok(Self {
            files,
            normalized_index_records,
        })
    }
}

pub(super) async fn validate_projection_root(
    state_root: &Path,
    snapshot: &ControlHostProjectionSnapshot,
    expected_records: &[HostProjectionSnapshotRecord],
    canonical: &CanonicalHostProjection,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<()> {
    let owner_root = state_root.join("plugin-host-manager");
    validate_exact_tree(&owner_root, canonical, limits).await?;
    let inventory = scan_host_projection_snapshot(
        state_root,
        &snapshot.manifest.binding.installation,
        limits.max_files,
        limits.max_payload_bytes,
    )
    .await
    .map_err(wrap_archive_error)?;
    let entries = inventory
        .sources
        .iter()
        .map(|source| ControlHostProjectionEntry {
            kind: source.kind.into(),
            path: source.logical_path.clone(),
            length: source.length,
            sha256: source.sha256.clone(),
        })
        .collect::<Vec<_>>();
    let records = inventory
        .sources
        .into_iter()
        .map(|source| source.record)
        .collect::<Vec<_>>();
    if entries != snapshot.manifest.entries
        || records != expected_records
        || inventory.validated_index_records != canonical.normalized_index_records
    {
        return Err(restore_invalid(
            "The canonical Host restore candidate differs from its exact semantic snapshot.",
        ));
    }
    Ok(())
}

async fn write_candidate_file(
    owner_root: &Path,
    logical_path: &str,
    bytes: &[u8],
    expected_sha256: &str,
) -> UseResult<()> {
    if bytes.is_empty() || sha256(bytes) != expected_sha256 {
        return Err(restore_invalid(
            "A Host restore candidate file differs from its expected bytes.",
        ));
    }
    let target = join_portable(owner_root, logical_path)?;
    let parent = target
        .parent()
        .ok_or_else(|| restore_invalid("A Host restore candidate file has no parent."))?
        .to_path_buf();
    ensure_owned_directory(owner_root, &parent).await?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| restore_invalid("A Host restore candidate file name is invalid."))?;
    let digest = expected_sha256.strip_prefix("sha256:").ok_or_else(|| {
        restore_invalid("A Host restore candidate digest has an invalid encoding.")
    })?;
    let partial = parent.join(format!(".{file_name}.{digest}.restore-partial"));
    let current = optional_regular_file_length(&target).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if let Some(length) = current {
        if partial_length.is_some()
            || length != bytes.len() as u64
            || read_exact_owned(&target, length).await? != bytes
        {
            return Err(restore_invalid(
                "An existing Host restore candidate file differs from its canonical bytes.",
            ));
        }
        return Ok(());
    }
    if let Some(length) = partial_length {
        if length == bytes.len() as u64 && read_exact_owned(&partial, length).await? == bytes {
            publish_noclobber(
                partial,
                target.clone(),
                "publish canonical Host restore record",
                false,
            )
            .await?;
            sync_directory(&parent).await?;
            return Ok(());
        }
        if length >= bytes.len() as u64 {
            return Err(restore_invalid(
                "A staged Host restore record has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete Host restore record", error))?;
        sync_directory(&parent).await?;
    }

    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create staged Host restore record", error))?;
    output
        .write_all(bytes)
        .await
        .map_err(|error| restore_io("write staged Host restore record", error))?;
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush staged Host restore record", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync staged Host restore record", error))?;
    drop(output);
    sync_directory(&parent).await?;
    if read_exact_owned(&partial, bytes.len() as u64).await? != bytes {
        return Err(restore_invalid(
            "A staged Host restore record changed before publication.",
        ));
    }
    publish_noclobber(
        partial,
        target.clone(),
        "publish canonical Host restore record",
        false,
    )
    .await?;
    sync_directory(&parent).await
}

async fn validate_exact_tree(
    owner_root: &Path,
    canonical: &CanonicalHostProjection,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<()> {
    if !optional_owned_directory(owner_root).await? {
        return Err(restore_invalid(
            "The canonical Host projection owner root is missing.",
        ));
    }
    let expected = canonical
        .files
        .iter()
        .map(|file| (file.logical_path.as_str(), file))
        .collect::<BTreeMap<_, _>>();
    let mut expected_directories = BTreeSet::new();
    for file in &canonical.files {
        let segments = file.logical_path.split('/').collect::<Vec<_>>();
        for end in 1..segments.len() {
            expected_directories.insert(segments[..end].join("/"));
        }
    }
    let mut pending = vec![(String::new(), owner_root.to_path_buf())];
    let mut observed = BTreeSet::new();
    let mut file_count = 0_u64;
    let mut byte_count = 0_u64;
    let max_entries = limits.max_files.saturating_mul(8).min(800_000);
    let mut entry_count = 0_u64;
    while let Some((prefix, directory)) = pending.pop() {
        let mut children = fs::read_dir(&directory)
            .await
            .map_err(|error| restore_io("read canonical Host restore directory", error))?;
        while let Some(child) = children
            .next_entry()
            .await
            .map_err(|error| restore_io("read canonical Host restore entry", error))?
        {
            entry_count = entry_count.checked_add(1).ok_or_else(|| {
                restore_invalid("The canonical Host restore entry count overflowed.")
            })?;
            if entry_count > max_entries {
                return Err(restore_invalid(
                    "The canonical Host restore tree exceeds its entry bound.",
                ));
            }
            let name = child.file_name().into_string().map_err(|_| {
                restore_invalid("Canonical Host restore paths must be valid UTF-8.")
            })?;
            let logical = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            let metadata = fs::symlink_metadata(child.path())
                .await
                .map_err(|error| restore_io("inspect canonical Host restore entry", error))?;
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) {
                return Err(restore_invalid(
                    "The canonical Host restore tree contains a link or reparse point.",
                ));
            }
            if metadata.is_dir() {
                if !expected_directories.contains(&logical) {
                    return Err(restore_invalid(
                        "The canonical Host restore tree contains an unknown directory.",
                    ));
                }
                pending.push((logical, child.path()));
                continue;
            }
            let Some(expected_file) = expected.get(logical.as_str()) else {
                return Err(restore_invalid(
                    "The canonical Host restore tree contains an unknown file.",
                ));
            };
            if !metadata.is_file() || metadata.len() != expected_file.length {
                return Err(restore_invalid(
                    "A canonical Host restore file has an unexpected type or length.",
                ));
            }
            let bytes = read_exact_owned(&child.path(), metadata.len()).await?;
            if sha256(&bytes) != expected_file.sha256 || !observed.insert(logical) {
                return Err(restore_invalid(
                    "A canonical Host restore file differs from its expected digest.",
                ));
            }
            file_count = file_count.checked_add(1).ok_or_else(|| {
                restore_invalid("The canonical Host restore file count overflowed.")
            })?;
            byte_count = byte_count.checked_add(metadata.len()).ok_or_else(|| {
                restore_invalid("The canonical Host restore byte count overflowed.")
            })?;
        }
    }
    if observed.len() != expected.len()
        || file_count > limits.max_files
        || byte_count > limits.max_payload_bytes
    {
        return Err(restore_invalid(
            "The canonical Host restore tree is incomplete or exceeds its owner bounds.",
        ));
    }
    Ok(())
}

fn join_portable(root: &Path, logical_path: &str) -> UseResult<PathBuf> {
    let mut path = root.to_path_buf();
    let mut segments = 0_usize;
    for segment in logical_path.split('/') {
        if segment.is_empty() || matches!(segment, "." | "..") || segment.contains(['\\', ':']) {
            return Err(restore_invalid(
                "A Host restore record has a nonportable logical path.",
            ));
        }
        path.push(segment);
        segments += 1;
    }
    if segments < 2 || !path.starts_with(root) {
        return Err(restore_invalid(
            "A Host restore record path escapes its owner root.",
        ));
    }
    Ok(path)
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
