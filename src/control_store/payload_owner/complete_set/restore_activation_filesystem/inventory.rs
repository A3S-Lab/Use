//! Typed live-root inventory for ordered complete restore activation.

use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::UseResult;
use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;
use tokio::fs;

use super::super::restore::{restore_activation_invalid, restore_activation_io, RestoreComponent};
use super::super::restore_activation::ControlInstallationRestoreActivation;
use super::super::restore_activation_storage::validate_directory;
use super::super::restore_filesystem;
use super::MARKER_PARTIAL_FILE;
use crate::control_store::filesystem::CONTROL_STORE_DATABASE_FILE;
use crate::control_store::payload_owner::ControlPayloadOwnerId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ActivationRootInventory {
    pub(super) marker: bool,
    pub(super) marker_partial: bool,
    pub(super) live_control: bool,
    live_operations: bool,
    live_payload_owners: BTreeSet<ControlPayloadOwnerId>,
}

impl ActivationRootInventory {
    pub(super) fn has_non_control_payload(&self) -> bool {
        self.live_operations || !self.live_payload_owners.is_empty()
    }
}

pub(super) async fn inspect_root(
    state_root: &Path,
    attempt: &Path,
) -> UseResult<ActivationRootInventory> {
    validate_directory(state_root, "target state root").await?;
    if attempt != state_root.join(restore_filesystem::ATTEMPT_DIRECTORY) {
        return Err(restore_activation_invalid(
            "The complete restore activation attempt is outside its fixed state-root location.",
        ));
    }
    let mut inventory = ActivationRootInventory {
        marker: false,
        marker_partial: false,
        live_control: false,
        live_operations: false,
        live_payload_owners: BTreeSet::new(),
    };
    let mut entries = fs::read_dir(state_root)
        .await
        .map_err(|error| restore_activation_io("read activation state root", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_activation_io("read activation state entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_activation_invalid("The activation state root contains a non-UTF-8 entry.")
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_activation_io("inspect activation state entry", error))?;
        let owned_file =
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_file();
        let valid = if crate::installation_state_layout::excluded_root_lock(&name) {
            owned_file
        } else if name == restore_filesystem::ATTEMPT_DIRECTORY {
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir()
        } else if name == ACTIVE_STATE_RESTORE_MARKER {
            inventory.marker = true;
            owned_file
        } else if name == MARKER_PARTIAL_FILE {
            inventory.marker_partial = true;
            owned_file
        } else if name == CONTROL_STORE_DATABASE_FILE {
            inventory.live_control = true;
            owned_file
        } else if let Some(owner) = ControlPayloadOwnerId::owner_for_state_root(&name) {
            inventory.live_payload_owners.insert(owner);
            !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir()
        } else if name == "operations" {
            inventory.live_operations = true;
            let owned_directory =
                !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir();
            if owned_directory {
                inspect_operations_root(&entry.path(), &mut inventory).await?;
            }
            owned_directory
        } else {
            false
        };
        if !valid {
            return Err(restore_activation_invalid(
                "The complete restore activation state root contains an unowned or linked entry.",
            ));
        }
    }
    Ok(inventory)
}

pub(super) fn validate_checkpoint_root(
    activation: &ControlInstallationRestoreActivation,
    inventory: &ActivationRootInventory,
) -> UseResult<()> {
    let checkpoints = activation.checkpoint_count();
    if checkpoints > 0 && !inventory.live_control {
        return Err(restore_activation_invalid(
            "The activation journal records Control completion without a live database.",
        ));
    }
    let owner_outside_prefix = inventory.live_payload_owners.iter().any(|owner| {
        RestoreComponent::for_payload_owner(*owner)
            .is_none_or(|component| checkpoints < component.index())
    });
    if owner_outside_prefix
        || inventory.live_operations && checkpoints < RestoreComponent::Observations.index()
    {
        return Err(restore_activation_invalid(
            "A live restore owner exists outside the activation journal's ordered prefix.",
        ));
    }
    Ok(())
}

pub(super) fn require_control_boundary(candidate_exists: bool, live_exists: bool) -> UseResult<()> {
    if candidate_exists == live_exists {
        return Err(restore_activation_invalid(
            "The complete restore Control boundary is ambiguous or missing.",
        ));
    }
    Ok(())
}

async fn inspect_operations_root(
    operations: &Path,
    inventory: &mut ActivationRootInventory,
) -> UseResult<()> {
    let mut entries = fs::read_dir(operations)
        .await
        .map_err(|error| restore_activation_io("read activation operations root", error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| restore_activation_io("read activation operations entry", error))?
    {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_activation_invalid("The activation operations root has a non-UTF-8 entry.")
        })?;
        let metadata = fs::symlink_metadata(entry.path())
            .await
            .map_err(|error| restore_activation_io("inspect activation operations entry", error))?;
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(restore_activation_invalid(
                "The activation operations root contains a linked or non-directory owner.",
            ));
        }
        let owner = ControlPayloadOwnerId::owner_for_operation_root(&name).ok_or_else(|| {
            restore_activation_invalid(
                "The activation operations root contains an unregistered owner.",
            )
        })?;
        inventory.live_payload_owners.insert(owner);
    }
    Ok(())
}
