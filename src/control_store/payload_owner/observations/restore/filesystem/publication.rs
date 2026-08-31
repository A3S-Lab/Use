use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    any_record_partial, archive_evidence, ensure_owned_directory, optional_regular_file,
    optional_regular_file_length, publish_noclobber, record_partial_path, restore_io,
    sync_directory, validate_staging_entries, wrap_archive_error,
};
use crate::control_store::payload_owner::observations::{
    archive, ControlObservationPayloadEntry, ControlObservationPayloadSnapshot,
};

use super::super::{restore_invalid, restore_target_not_empty};

pub(in crate::control_store::payload_owner::observations::restore) async fn activate_archive(
    archive_path: &Path,
    staging_directory: &Path,
    state_root: &Path,
    snapshot: &ControlObservationPayloadSnapshot,
    existing: &[ControlObservationPayloadEntry],
) -> UseResult<()> {
    let (expected_bytes, expected_digest) = archive_evidence(snapshot)?;
    let mut reader = archive::ObservationArchiveReader::open(
        archive_path,
        expected_bytes,
        expected_digest,
        &snapshot.manifest.entries,
        &snapshot.manifest.binding.installation,
    )
    .await
    .map_err(wrap_archive_error)?;
    let existing = existing
        .iter()
        .map(|entry| entry.path.as_str())
        .collect::<BTreeSet<_>>();
    let mut index = 0_usize;
    while let Some((entry, bytes)) = reader.next().await.map_err(wrap_archive_error)? {
        let partial = record_partial_path(staging_directory, index, &entry);
        if existing.contains(entry.path.as_str()) {
            reconcile_published_partial(&partial, &entry).await?;
        } else {
            publish_record(state_root, staging_directory, &partial, &entry, &bytes).await?;
        }
        index += 1;
    }
    reader.finish().await.map_err(wrap_archive_error)?;
    validate_staging_entries(staging_directory, snapshot).await?;
    if any_record_partial(staging_directory, snapshot).await? {
        return Err(restore_invalid(
            "A restored observation left an incomplete staged record.",
        ));
    }
    Ok(())
}

async fn publish_record(
    state_root: &Path,
    staging_directory: &Path,
    partial: &Path,
    entry: &ControlObservationPayloadEntry,
    bytes: &[u8],
) -> UseResult<()> {
    let target = live_record_path(state_root, &entry.path);
    let parent = target
        .parent()
        .ok_or_else(|| restore_invalid("A restored observation record has no parent."))?;
    ensure_owned_directory(state_root, parent).await?;
    if optional_regular_file(&target).await? {
        return Err(restore_target_not_empty());
    }
    prepare_record_partial(partial, entry, bytes, staging_directory).await?;
    publish_noclobber(
        partial.to_path_buf(),
        target.clone(),
        "publish restored observation record",
        true,
    )
    .await?;
    sync_directory(parent).await?;
    sync_directory(staging_directory).await
}

async fn prepare_record_partial(
    partial: &Path,
    entry: &ControlObservationPayloadEntry,
    bytes: &[u8],
    staging_directory: &Path,
) -> UseResult<()> {
    if let Some(length) = optional_regular_file_length(partial).await? {
        if length == entry.length {
            if read_exact_owned(partial, entry.length).await? == bytes {
                return Ok(());
            }
            return Err(restore_invalid(
                "A complete staged observation record differs from its archive entry.",
            ));
        }
        if length > entry.length {
            return Err(restore_invalid(
                "A staged observation record exceeds its archive entry.",
            ));
        }
        fs::remove_file(partial)
            .await
            .map_err(|error| restore_io("remove incomplete staged observation record", error))?;
        sync_directory(staging_directory).await?;
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(partial)
        .await
        .map_err(|error| restore_io("create staged observation record", error))?;
    output
        .write_all(bytes)
        .await
        .map_err(|error| restore_io("write staged observation record", error))?;
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush staged observation record", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync staged observation record", error))?;
    drop(output);
    sync_directory(staging_directory).await?;
    if read_exact_owned(partial, entry.length).await? != bytes {
        return Err(restore_invalid(
            "The staged observation record changed before publication.",
        ));
    }
    Ok(())
}

async fn reconcile_published_partial(
    partial: &Path,
    entry: &ControlObservationPayloadEntry,
) -> UseResult<()> {
    let Some(length) = optional_regular_file_length(partial).await? else {
        return Ok(());
    };
    let bytes = read_exact_owned(partial, length).await?;
    if length != entry.length || sha256(&bytes) != entry.sha256 {
        return Err(restore_invalid(
            "A residual staged observation record differs from its published entry.",
        ));
    }
    fs::remove_file(partial)
        .await
        .map_err(|error| restore_io("remove published staged observation record", error))?;
    if let Some(parent) = partial.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn read_exact_owned(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    let (mut file, metadata) = archive::open_owned_regular_file(path, "staged observation record")
        .await
        .map_err(wrap_archive_error)?;
    if metadata.len() != expected_length {
        return Err(restore_invalid(
            "A staged observation record changed before it was read.",
        ));
    }
    let capacity = usize::try_from(expected_length)
        .map_err(|_| restore_invalid("A staged observation record length is invalid."))?;
    let mut bytes = Vec::with_capacity(capacity);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_io("read staged observation record", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(restore_invalid(
            "A staged observation record changed while it was read.",
        ));
    }
    Ok(bytes)
}

fn live_record_path(state_root: &Path, portable: &str) -> PathBuf {
    let mut path = state_root.join("operations");
    for segment in portable.split('/') {
        path.push(segment);
    }
    path
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
