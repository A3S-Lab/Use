use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    optional_regular_file_length, publish_noclobber, restore_io, sync_directory,
    validate_staging_entries, wrap_archive_error, ARCHIVE_FILE, ARCHIVE_PARTIAL_FILE,
};
use crate::control_store::payload_owner::host_projection::{
    archive, restore::restore_invalid, ControlHostProjectionSnapshot, ControlHostProjectionState,
};

pub(in crate::control_store::payload_owner::host_projection::restore) async fn stage_archive(
    source: &Path,
    staging_directory: &Path,
    snapshot: &ControlHostProjectionSnapshot,
) -> UseResult<PathBuf> {
    validate_staging_entries(staging_directory).await?;
    let candidate = staging_directory.join(ARCHIVE_FILE);
    let partial = staging_directory.join(ARCHIVE_PARTIAL_FILE);
    let candidate_length = optional_regular_file_length(&candidate).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if candidate_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The Host projection restore archive state is ambiguous.",
        ));
    }
    if candidate_length.is_some() {
        verify_archive(snapshot, &candidate).await?;
        return Ok(candidate);
    }
    let expected_bytes = archive_bytes(snapshot)?;
    if let Some(length) = partial_length {
        if length == expected_bytes && verify_archive(snapshot, &partial).await.is_ok() {
            publish_noclobber(
                partial,
                candidate.clone(),
                "publish Host projection restore archive",
                false,
            )
            .await?;
            sync_directory(staging_directory).await?;
            return Ok(candidate);
        }
        if length >= expected_bytes {
            return Err(restore_invalid(
                "The partial Host projection restore archive has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete Host restore archive", error))?;
        sync_directory(staging_directory).await?;
    }

    verify_archive(snapshot, source).await?;
    let (mut input, metadata) = archive::open_owned_regular_file(source, "Host restore source")
        .await
        .map_err(wrap_archive_error)?;
    if metadata.len() != expected_bytes {
        return Err(restore_invalid(
            "The verified Host projection restore source changed before staging.",
        ));
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create partial Host restore archive", error))?;
    let copied = tokio::io::copy(
        &mut (&mut input).take(expected_bytes.saturating_add(1)),
        &mut output,
    )
    .await
    .map_err(|error| restore_io("copy Host projection restore archive", error))?;
    if copied != expected_bytes {
        return Err(restore_invalid(
            "The verified Host projection restore source changed while it was staged.",
        ));
    }
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush Host restore archive", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Host restore archive", error))?;
    drop(output);
    verify_archive(snapshot, &partial).await?;
    publish_noclobber(
        partial,
        candidate.clone(),
        "publish Host projection restore archive",
        false,
    )
    .await?;
    sync_directory(staging_directory).await?;
    Ok(candidate)
}

pub(in crate::control_store::payload_owner::host_projection::restore) async fn staged_archive(
    staging_directory: &Path,
    snapshot: &ControlHostProjectionSnapshot,
) -> UseResult<PathBuf> {
    validate_staging_entries(staging_directory).await?;
    let candidate = staging_directory.join(ARCHIVE_FILE);
    if optional_regular_file_length(&staging_directory.join(ARCHIVE_PARTIAL_FILE))
        .await?
        .is_some()
        || optional_regular_file_length(&candidate).await?.is_none()
    {
        return Err(restore_invalid(
            "The staged Host projection restore archive is incomplete.",
        ));
    }
    verify_archive(snapshot, &candidate).await?;
    Ok(candidate)
}

async fn verify_archive(snapshot: &ControlHostProjectionSnapshot, path: &Path) -> UseResult<()> {
    archive::verify_archive(snapshot, Some(path))
        .await
        .map(|_| ())
        .map_err(wrap_archive_error)
}

fn archive_bytes(snapshot: &ControlHostProjectionSnapshot) -> UseResult<u64> {
    match snapshot.manifest.payload {
        ControlHostProjectionState::Archive { archive_bytes, .. } => Ok(archive_bytes),
        ControlHostProjectionState::Absent => Err(restore_invalid(
            "An absent Host projection snapshot has no archive evidence.",
        )),
    }
}
