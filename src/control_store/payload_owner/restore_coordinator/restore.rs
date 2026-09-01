//! Qualification-only restoration for the coordinator's own journal.
//!
//! The active restore marker is preserved while terminal history is reconciled
//! through durable, replayable staging evidence. Standalone qualification binds
//! the legacy whole-installation marker; complete-set activation instead binds
//! the exact typed complete marker, which has no retained operation or future
//! history slot. This module is not wired into production backup or restore
//! orchestration.

use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use sha2::{Digest, Sha256};

use super::{
    ControlPayloadOwnerRegistry, ControlRestoreCoordinatorSnapshot, ControlRestoreCoordinatorState,
    VerifiedControlRestoreCoordinatorSnapshot,
};

mod evidence;
mod filesystem;

pub(in crate::control_store) use evidence::ControlRestoreCoordinatorRestoreResult;
#[cfg(test)]
pub(in crate::control_store) use evidence::ControlRestoreCoordinatorRestoreState;

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlRestoreCoordinatorRestore {
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlRestoreCoordinatorSnapshot,
    state_root: PathBuf,
    staging_directory: PathBuf,
    candidate: Option<PathBuf>,
}

impl VerifiedControlRestoreCoordinatorSnapshot {
    pub(in crate::control_store) async fn stage_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
    ) -> UseResult<StagedControlRestoreCoordinatorRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        filesystem::validate_staging_location(&state_root, &staging_directory)?;
        let _maintenance = StateMaintenanceLock::new(&state_root)
            .acquire_shared()
            .await
            .map_err(wrap_restore_error)?;
        self.stage_restore_inner(state_root, staging_directory)
            .await
    }

    pub(in crate::control_store) async fn stage_restore_under_exclusive(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlRestoreCoordinatorRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        filesystem::validate_staging_location(&state_root, &staging_directory)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Restore Coordinator staging requires the exact target's exclusive maintenance guard.",
            ));
        }
        self.stage_restore_inner(state_root, staging_directory)
            .await
    }

    async fn stage_restore_inner(
        &self,
        state_root: PathBuf,
        staging_directory: PathBuf,
    ) -> UseResult<StagedControlRestoreCoordinatorRestore> {
        filesystem::ensure_owned_directory(&state_root, &staging_directory).await?;
        filesystem::require_pre_activation_staging(&staging_directory, &self.snapshot).await?;

        let candidate = match (
            &self.snapshot.manifest.payload,
            self.archive_path.as_deref(),
        ) {
            (ControlRestoreCoordinatorState::Absent, None) => {
                filesystem::require_empty_staging(&staging_directory).await?;
                None
            }
            (ControlRestoreCoordinatorState::Archive { .. }, Some(archive_path)) => {
                filesystem::prepare_candidate(archive_path, &staging_directory, &self.snapshot)
                    .await?;
                Some(filesystem::candidate_path(&staging_directory))
            }
            _ => {
                return Err(restore_invalid(
                    "The verified Restore Coordinator snapshot omitted or added archive bytes.",
                ))
            }
        };
        filesystem::require_pre_activation_staging(&staging_directory, &self.snapshot).await?;
        Ok(StagedControlRestoreCoordinatorRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            state_root,
            staging_directory,
            candidate,
        })
    }

    pub(in crate::control_store) async fn reopen_staged_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlRestoreCoordinatorRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        filesystem::validate_staging_location(&state_root, &staging_directory)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Restore Coordinator replay requires the exact target's exclusive maintenance guard.",
            ));
        }
        filesystem::validate_owned_directory_chain(&state_root, &staging_directory).await?;
        filesystem::validate_staging_entries(&staging_directory, &self.snapshot).await?;
        let candidate = match &self.snapshot.manifest.payload {
            ControlRestoreCoordinatorState::Absent => {
                filesystem::require_candidate_absent(&staging_directory).await?;
                None
            }
            ControlRestoreCoordinatorState::Archive { .. } => {
                filesystem::inspect_candidate(&staging_directory, &self.snapshot).await?;
                Some(filesystem::candidate_path(&staging_directory))
            }
        };
        Ok(StagedControlRestoreCoordinatorRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            state_root,
            staging_directory,
            candidate,
        })
    }
}

impl StagedControlRestoreCoordinatorRestore {
    pub(in crate::control_store) fn candidate_path(&self) -> Option<&Path> {
        self.candidate.as_deref()
    }

    pub(in crate::control_store) async fn preflight_clean(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<()> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Restore Coordinator preflight requires the exact target's exclusive maintenance guard.",
            ));
        }
        filesystem::preflight_clean(&self.state_root, &self.staging_directory, &self.snapshot).await
    }

    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlRestoreCoordinatorRestoreResult> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Restore Coordinator activation requires the exact target's exclusive maintenance guard.",
            ));
        }
        let activation =
            filesystem::activate(&self.state_root, &self.staging_directory, &self.snapshot).await?;
        ControlRestoreCoordinatorRestoreResult::new(&self.registry, &self.snapshot, &activation)
    }

    pub(in crate::control_store) async fn activate_for_complete_restore(
        &self,
        maintenance: &StateMaintenanceGuard,
        expected_plan_digest: &str,
        expected_marker_bytes: &[u8],
    ) -> UseResult<ControlRestoreCoordinatorRestoreResult> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Restore Coordinator activation requires the exact target's exclusive maintenance guard.",
            ));
        }
        if expected_marker_bytes.is_empty()
            || expected_marker_bytes.len() as u64 > filesystem::MAX_ACTIVE_MARKER_BYTES
        {
            return Err(restore_invalid(
                "The complete restore marker evidence exceeds the Restore Coordinator bound.",
            ));
        }
        let marker_sha256 = format!("sha256:{:x}", Sha256::digest(expected_marker_bytes));
        let activation = filesystem::activate_bound(
            &self.state_root,
            &self.staging_directory,
            &self.snapshot,
            filesystem::ExpectedActiveRestore {
                plan_digest: expected_plan_digest,
                marker_length: expected_marker_bytes.len() as u64,
                marker_sha256: &marker_sha256,
            },
        )
        .await?;
        ControlRestoreCoordinatorRestoreResult::new(&self.registry, &self.snapshot, &activation)
    }
}

fn wrap_restore_error(error: UseError) -> UseError {
    restore_invalid(format!(
        "Restore Coordinator restore verification failed: {}",
        error.message
    ))
}

fn restore_requires_active() -> UseError {
    UseError::new(
        "use.control_store.restore_coordinator_restore_requires_active",
        "Restore Coordinator history replacement requires an active whole-installation restore marker.",
    )
}

fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.restore_coordinator_restore_invalid",
        message,
    )
}
