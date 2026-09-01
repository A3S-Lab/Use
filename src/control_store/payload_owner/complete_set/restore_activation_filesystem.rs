//! Owned filesystem protocol for the complete restore activation journal.

use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;
use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;
use tokio::fs;

use super::restore::{restore_activation_invalid, restore_activation_io};
use super::restore_activation::{maybe_test_crash, ControlInstallationRestoreActivation};
use super::restore_activation_storage::{
    optional_regular_file, optional_regular_file_length, publish_noclobber, read_bounded_file,
    remove_obsolete_temporary, sync_directory, write_synced_new,
};
use super::restore_filesystem;
use crate::control_store::filesystem::CONTROL_STORE_DATABASE_FILE;
use crate::state_restore::ControlInstallationRestoreActiveMarker;

mod inventory;

use inventory::{
    inspect_root, require_control_boundary, validate_checkpoint_root, ActivationRootInventory,
};

pub(super) const ACTIVATION_FILE: &str = "activation.json";
pub(super) const ACTIVATION_TEMPORARY_FILE: &str = "activation.json.tmp";
const MARKER_PARTIAL_FILE: &str = ".maintenance.restore.json.partial";
pub(super) const MAX_ACTIVATION_BYTES: u64 = 128 * 1024;
pub(super) const MAX_MARKER_BYTES: u64 = 4 * 1024;

pub(super) fn journal_path(attempt: &Path) -> PathBuf {
    attempt.join(ACTIVATION_FILE)
}

pub(super) async fn journal_exists(attempt: &Path) -> UseResult<bool> {
    optional_regular_file(&journal_path(attempt)).await
}

pub(super) async fn marker_exists(state_root: &Path) -> UseResult<bool> {
    optional_regular_file(&state_root.join(ACTIVE_STATE_RESTORE_MARKER)).await
}

pub(super) async fn load_or_begin(
    state_root: &Path,
    attempt: &Path,
    attempt_bytes: &[u8],
    attempt_digest: &str,
    control_candidate: &Path,
) -> UseResult<ControlInstallationRestoreActivation> {
    let expected_candidate =
        restore_filesystem::component_directory(attempt, restore_filesystem::CONTROL_DIRECTORY)
            .join(CONTROL_STORE_DATABASE_FILE);
    if control_candidate != expected_candidate {
        return Err(restore_activation_invalid(
            "The complete restore activation Control candidate is outside its fixed component path.",
        ));
    }
    restore_filesystem::validate_attempt(attempt, attempt_bytes).await?;
    let inventory = inspect_root(state_root, attempt).await?;
    let candidate_exists = optional_regular_file(control_candidate).await?;
    require_control_boundary(candidate_exists, inventory.live_control)?;
    recover_journal_temporary(attempt, attempt_digest).await?;
    let activation = match read_journal(attempt, attempt_digest).await? {
        Some(activation) => activation,
        None => {
            if inventory.marker
                || inventory.marker_partial
                || inventory.live_control
                || inventory.has_non_control_payload()
                || !candidate_exists
            {
                return Err(restore_activation_invalid(
                    "Complete restore activation state exists without its durable journal.",
                ));
            }
            let activation = ControlInstallationRestoreActivation::new(attempt_digest)?;
            publish_new_journal(attempt, &activation).await?;
            maybe_test_crash("journal-published");
            activation
        }
    };
    let marker = activation.active_marker()?;
    reconcile_marker(
        state_root,
        &activation,
        &marker,
        candidate_exists,
        inventory,
    )
    .await?;
    maybe_test_crash("marker-published");
    let durable = load_active(state_root, attempt, attempt_digest).await?;
    if durable.operation_digest() != activation.operation_digest() {
        return Err(restore_activation_invalid(
            "The complete restore activation identity changed while its marker was published.",
        ));
    }
    Ok(durable)
}

pub(super) async fn load_active(
    state_root: &Path,
    attempt: &Path,
    attempt_digest: &str,
) -> UseResult<ControlInstallationRestoreActivation> {
    let inventory = inspect_root(state_root, attempt).await?;
    let candidate =
        restore_filesystem::component_directory(attempt, restore_filesystem::CONTROL_DIRECTORY)
            .join(CONTROL_STORE_DATABASE_FILE);
    let candidate_exists = optional_regular_file(&candidate).await?;
    require_control_boundary(candidate_exists, inventory.live_control)?;
    if inventory.marker_partial {
        return Err(restore_activation_invalid(
            "The complete restore activation has an ambiguous partial active marker.",
        ));
    }
    recover_journal_temporary(attempt, attempt_digest).await?;
    let activation = read_journal(attempt, attempt_digest)
        .await?
        .ok_or_else(|| {
            restore_activation_invalid(
                "The active complete restore marker has no durable activation journal.",
            )
        })?;
    if inventory.marker {
        let expected = activation.active_marker()?;
        read_exact_marker(state_root, &activation, &expected).await?;
    } else if !activation.is_complete() {
        return Err(restore_activation_invalid(
            "The incomplete restore activation has no exact durable active marker.",
        ));
    }
    validate_checkpoint_root(&activation, &inventory)?;
    Ok(activation)
}

pub(super) async fn replace_journal(
    attempt: &Path,
    current: &ControlInstallationRestoreActivation,
    next: &ControlInstallationRestoreActivation,
) -> UseResult<()> {
    let target = journal_path(attempt);
    let current_bytes = current.canonical_bytes()?;
    let existing = read_bounded_file(&target, MAX_ACTIVATION_BYTES, "activation journal").await?;
    if existing != current_bytes {
        return Err(restore_activation_invalid(
            "The complete restore activation journal changed before checkpoint publication.",
        ));
    }
    let temporary = attempt.join(ACTIVATION_TEMPORARY_FILE);
    remove_obsolete_temporary(&temporary, MAX_ACTIVATION_BYTES).await?;
    write_synced_new(
        &temporary,
        &next.canonical_bytes()?,
        "activation checkpoint",
    )
    .await?;
    let source = temporary.clone();
    let target_for_worker = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_replace_blocking(source, &target_for_worker)
    })
    .await
    .map_err(|error| {
        restore_activation_invalid(format!(
            "The complete restore checkpoint publication worker did not complete: {error}"
        ))
    })?
    .map_err(|error| restore_activation_io("publish complete restore checkpoint", error))?;
    sync_directory(attempt).await?;
    let durable = read_bounded_file(&target, MAX_ACTIVATION_BYTES, "activation journal").await?;
    if durable != next.canonical_bytes()? {
        return Err(restore_activation_invalid(
            "The complete restore checkpoint changed while it was published.",
        ));
    }
    Ok(())
}

async fn reconcile_marker(
    state_root: &Path,
    activation: &ControlInstallationRestoreActivation,
    expected: &ControlInstallationRestoreActiveMarker,
    candidate_exists: bool,
    inventory: ActivationRootInventory,
) -> UseResult<()> {
    if inventory.marker && inventory.marker_partial {
        return Err(restore_activation_invalid(
            "The active complete restore marker state is ambiguous.",
        ));
    }
    if inventory.marker {
        read_exact_marker(state_root, activation, expected).await?;
        validate_checkpoint_root(activation, &inventory)?;
        return Ok(());
    }
    if activation.is_complete() {
        validate_checkpoint_root(activation, &inventory)?;
        return Ok(());
    }
    if activation.checkpoint_count() != 0 || inventory.live_control || !candidate_exists {
        return Err(restore_activation_invalid(
            "The active complete restore marker is missing after activation effects began.",
        ));
    }

    let marker = state_root.join(ACTIVE_STATE_RESTORE_MARKER);
    let partial = state_root.join(MARKER_PARTIAL_FILE);
    let expected_bytes = marker_bytes(expected)?;
    if inventory.marker_partial {
        if read_bounded_file(&partial, MAX_MARKER_BYTES, "partial active restore marker").await?
            != expected_bytes
        {
            return Err(restore_activation_invalid(
                "The partial active complete restore marker was changed or rebound.",
            ));
        }
    } else {
        write_synced_new(&partial, &expected_bytes, "partial active restore marker").await?;
    }
    publish_noclobber(partial, marker, "active complete restore marker").await?;
    sync_directory(state_root).await
}

async fn publish_new_journal(
    attempt: &Path,
    activation: &ControlInstallationRestoreActivation,
) -> UseResult<()> {
    let target = journal_path(attempt);
    let temporary = attempt.join(ACTIVATION_TEMPORARY_FILE);
    write_synced_new(
        &temporary,
        &activation.canonical_bytes()?,
        "initial activation journal",
    )
    .await?;
    publish_noclobber(temporary, target, "initial activation journal").await?;
    sync_directory(attempt).await
}

async fn recover_journal_temporary(attempt: &Path, attempt_digest: &str) -> UseResult<()> {
    let target = journal_path(attempt);
    let temporary = attempt.join(ACTIVATION_TEMPORARY_FILE);
    let target_exists = optional_regular_file(&target).await?;
    let Some(_) = optional_regular_file_length(&temporary).await? else {
        return Ok(());
    };
    if target_exists {
        return remove_obsolete_temporary(&temporary, MAX_ACTIVATION_BYTES).await;
    }
    let bytes = read_bounded_file(
        &temporary,
        MAX_ACTIVATION_BYTES,
        "temporary activation journal",
    )
    .await?;
    let activation = decode_journal(&bytes, attempt_digest)?;
    if activation.checkpoint_count() != 0 {
        return Err(restore_activation_invalid(
            "A checkpointed activation temporary exists without its durable journal.",
        ));
    }
    publish_noclobber(temporary, target, "recovered activation journal").await?;
    sync_directory(attempt).await
}

pub(super) async fn read_journal(
    attempt: &Path,
    attempt_digest: &str,
) -> UseResult<Option<ControlInstallationRestoreActivation>> {
    let path = journal_path(attempt);
    let Some(_) = optional_regular_file_length(&path).await? else {
        return Ok(None);
    };
    let bytes = read_bounded_file(&path, MAX_ACTIVATION_BYTES, "activation journal").await?;
    decode_journal(&bytes, attempt_digest).map(Some)
}

fn decode_journal(
    bytes: &[u8],
    attempt_digest: &str,
) -> UseResult<ControlInstallationRestoreActivation> {
    ControlInstallationRestoreActivation::decode_canonical(bytes, attempt_digest)
}

async fn read_exact_marker(
    state_root: &Path,
    activation: &ControlInstallationRestoreActivation,
    expected: &ControlInstallationRestoreActiveMarker,
) -> UseResult<()> {
    let bytes = read_bounded_file(
        &state_root.join(ACTIVE_STATE_RESTORE_MARKER),
        MAX_MARKER_BYTES,
        "active complete restore marker",
    )
    .await?;
    let marker: ControlInstallationRestoreActiveMarker = serde_json::from_slice(&bytes)
        .map_err(|_| restore_activation_invalid("The active restore marker is invalid JSON."))?;
    let expected_bytes = marker_bytes(expected)?;
    marker
        .validate_exact(activation.attempt_digest(), activation.operation_digest())
        .map_err(|error| {
            restore_activation_invalid(format!(
                "The active complete restore marker is invalid or was rebound: {}",
                error.message
            ))
        })?;
    if bytes != expected_bytes {
        return Err(restore_activation_invalid(
            "The active complete restore marker was changed or rebound.",
        ));
    }
    Ok(())
}

pub(super) async fn retire_marker(
    state_root: &Path,
    attempt: &Path,
    attempt_digest: &str,
) -> UseResult<ControlInstallationRestoreActivation> {
    let activation = load_active(state_root, attempt, attempt_digest).await?;
    if !activation.is_complete() {
        return Err(restore_activation_invalid(
            "The active complete restore marker cannot retire before every owner checkpoint.",
        ));
    }
    let inventory = inspect_root(state_root, attempt).await?;
    if inventory.marker_partial {
        return Err(restore_activation_invalid(
            "The completed restore has an ambiguous partial active marker.",
        ));
    }
    if inventory.marker {
        let expected = activation.active_marker()?;
        read_exact_marker(state_root, &activation, &expected).await?;
        fs::remove_file(state_root.join(ACTIVE_STATE_RESTORE_MARKER))
            .await
            .map_err(|error| {
                restore_activation_io("retire active complete restore marker", error)
            })?;
        sync_directory(state_root).await?;
        maybe_test_crash("marker-retired");
    }
    let retired = inspect_root(state_root, attempt).await?;
    if retired.marker || retired.marker_partial {
        return Err(restore_activation_invalid(
            "The completed restore marker did not retire exactly.",
        ));
    }
    load_active(state_root, attempt, attempt_digest).await
}

fn marker_bytes(marker: &ControlInstallationRestoreActiveMarker) -> UseResult<Vec<u8>> {
    let bytes = marker.canonical_bytes().map_err(|error| {
        restore_activation_invalid(format!(
            "Failed to encode the active complete restore marker: {}",
            error.message
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_MARKER_BYTES {
        return Err(restore_activation_invalid(
            "The active complete restore marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}
