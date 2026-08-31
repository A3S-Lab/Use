use std::path::Path;

use a3s_use_core::UseResult;
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::super::{
    activation_partial_path, activation_path, optional_regular_file_length, publish_noclobber,
    read_owned_file, restore_io, sync_directory,
};
use crate::control_store::payload_owner::restore_coordinator::restore::evidence::{
    entries_from_scan, RestoreCoordinatorActivation, MAX_ACTIVATION_BYTES,
};
use crate::control_store::payload_owner::restore_coordinator::restore::restore_invalid;
use crate::control_store::payload_owner::restore_coordinator::{
    ControlRestoreCoordinatorEntry, ControlRestoreCoordinatorSnapshot,
};
use crate::state_restore::StateRestoreHistorySnapshotScan;

pub(super) async fn load_or_create(
    staging_directory: &Path,
    snapshot: &ControlRestoreCoordinatorSnapshot,
    live: &StateRestoreHistorySnapshotScan,
    target: &[ControlRestoreCoordinatorEntry],
    pruned: Option<&str>,
) -> UseResult<(RestoreCoordinatorActivation, bool)> {
    let active = super::require_active(live)?;
    let marker = activation_path(staging_directory);
    let partial = activation_partial_path(staging_directory);
    let marker_length = optional_regular_file_length(&marker).await?;
    let partial_length = optional_regular_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The Restore Coordinator activation marker state is ambiguous.",
        ));
    }
    if marker_length.is_some() {
        let bytes = read_owned_file(
            &marker,
            MAX_ACTIVATION_BYTES,
            "Restore Coordinator activation marker",
        )
        .await?;
        let activation = RestoreCoordinatorActivation::decode_canonical(&bytes)?;
        activation.validate_for_snapshot(snapshot, target, pruned)?;
        if !activation.binds_active(active) {
            return Err(restore_invalid(
                "The Restore Coordinator activation marker binds another active restore.",
            ));
        }
        return Ok((activation, false));
    }

    let activation = RestoreCoordinatorActivation::new(
        snapshot,
        active,
        entries_from_scan(&live.terminal),
        target.to_vec(),
        pruned.map(str::to_owned),
    )?;
    activation.validate_for_snapshot(snapshot, target, pruned)?;
    let bytes = activation.canonical_bytes()?;
    if let Some(length) = partial_length {
        if length == bytes.len() as u64
            && read_owned_file(
                &partial,
                MAX_ACTIVATION_BYTES,
                "partial Restore Coordinator activation marker",
            )
            .await?
                == bytes
        {
            publish_noclobber(
                partial,
                marker,
                "publish Restore Coordinator activation marker",
            )
            .await?;
            sync_directory(staging_directory).await?;
            return Ok((activation, true));
        }
        if length >= bytes.len() as u64 {
            return Err(restore_invalid(
                "A partial Restore Coordinator activation marker has unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete activation marker", error))?;
        sync_directory(staging_directory).await?;
    }
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| restore_io("create Restore Coordinator activation marker", error))?;
    output
        .write_all(&bytes)
        .await
        .map_err(|error| restore_io("write Restore Coordinator activation marker", error))?;
    output
        .flush()
        .await
        .map_err(|error| restore_io("flush Restore Coordinator activation marker", error))?;
    output
        .sync_all()
        .await
        .map_err(|error| restore_io("sync Restore Coordinator activation marker", error))?;
    drop(output);
    sync_directory(staging_directory).await?;
    if read_owned_file(
        &partial,
        MAX_ACTIVATION_BYTES,
        "partial Restore Coordinator activation marker",
    )
    .await?
        != bytes
    {
        return Err(restore_invalid(
            "The Restore Coordinator activation marker changed before publication.",
        ));
    }
    publish_noclobber(
        partial,
        marker,
        "publish Restore Coordinator activation marker",
    )
    .await?;
    sync_directory(staging_directory).await?;
    Ok((activation, true))
}
