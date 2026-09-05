//! Inactive A2 Control Store kernel.
//!
//! ADR-003 permits this backend to be qualified before the coordinated
//! authority cutover. Nothing in the production package lifecycle constructs
//! it yet, so the current JSON stores remain the only authority and no dual
//! write path exists.

use std::path::PathBuf;

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};

mod aggregate;
mod composition;
mod dispatcher;
mod effect_owner;
mod effect_port;
mod executor;
mod export;
mod filesystem;
mod model;
mod operation_admission;
mod payload_owner;
mod schema;

use executor::ControlStoreExecutor;
use export::VerifiedControlStoreExport;
use filesystem::CONTROL_STORE_DATABASE_FILE;
use model::{
    ClaimedControlEffect, ControlEffectClaim, ControlEffectObservation, ControlEffectRecord,
    ControlGeneration, ControlOperationRecord, ControlPublishedCapabilityCursor, ControlTransition,
    ReviewedControlOperation,
};
use payload_owner::{ControlPayloadOwnerRegistry, ControlPayloadSnapshotSession};
use schema::{ControlStoreInspection, ControlStoreMetadata};

#[allow(unused_imports)]
pub(in crate::control_store) use composition::{
    ControlEffectCompositionDependencies, ControlStoreRuntimeComposition,
};

pub(crate) fn validate_terminal_restore_receipt_blocking(
    attempt: &std::path::Path,
) -> UseResult<InstallationId> {
    payload_owner::validate_terminal_receipt_blocking(attempt)
}

pub(crate) fn validate_capability_descriptor_snapshot_backup_bytes(
    bytes: &[u8],
    installation: &InstallationId,
    expected_digest: &str,
) -> UseResult<()> {
    effect_owner::validate_descriptor_snapshot_backup_bytes(bytes, installation, expected_digest)
}

pub(crate) fn capability_descriptor_snapshot_backup_installation(
    bytes: &[u8],
) -> UseResult<InstallationId> {
    effect_owner::descriptor_snapshot_backup_installation(bytes)
}

#[cfg(test)]
use executor::MAX_QUEUED_CONTROL_STORE_OPERATIONS;
#[cfg(test)]
use schema::{CONTROL_STORE_SCHEMA_VERSION, SQLITE_SYNCHRONOUS_FULL};

/// Installation-bound SQLite kernel kept inactive until the ADR-003 cutover.
///
/// Construction is side-effect free except for starting one bounded worker.
/// `initialize` is the only operation that may create a database, and it
/// rejects every existing legacy authority before doing so.
#[derive(Debug, Clone)]
struct ControlStore {
    installation: InstallationId,
    state_root: PathBuf,
    owned_roots: Vec<PathBuf>,
    database_path: PathBuf,
    executor: ControlStoreExecutor,
}

impl ControlStore {
    fn new(state_root: impl Into<PathBuf>, installation: InstallationId) -> UseResult<Self> {
        installation.validate()?;
        let state_root = state_root.into();
        let database_path = state_root.join(CONTROL_STORE_DATABASE_FILE);
        let executor = ControlStoreExecutor::new()?;
        Ok(Self {
            installation,
            owned_roots: vec![state_root.clone()],
            state_root,
            database_path,
            executor,
        })
    }

    fn from_extension_paths(paths: &ExtensionPaths) -> UseResult<Self> {
        let mut store = Self::new(
            paths.installation_state_root(),
            paths.installation().clone(),
        )?;
        store.owned_roots = vec![
            paths.use_paths().data_root().to_path_buf(),
            paths.use_paths().state_root().to_path_buf(),
        ];
        Ok(store)
    }

    async fn initialize(&self) -> UseResult<ControlStoreMetadata> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_exclusive()
            .await?;
        filesystem::prepare_initialization(&self.state_root, &self.database_path).await?;
        let physical_database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let metadata = self
            .executor
            .initialize(physical_database_path, self.installation.clone())
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(metadata)
    }

    async fn inspect(&self) -> UseResult<ControlStoreInspection> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let physical_database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let inspection = self
            .executor
            .inspect(physical_database_path, self.installation.clone())
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(inspection)
    }

    async fn export(&self) -> UseResult<Vec<u8>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let physical_database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let export = self
            .executor
            .export(physical_database_path, self.installation.clone())
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(export.into_bytes())
    }

    /// Freeze one canonical Control export while retaining the installation's
    /// exclusive maintenance fence for owner-specific payload snapshots.
    async fn begin_payload_snapshot(
        &self,
        registry: ControlPayloadOwnerRegistry,
    ) -> UseResult<ControlPayloadSnapshotSession> {
        registry.validate()?;
        let maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_exclusive()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let physical_database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let export = self
            .executor
            .export(physical_database_path, self.installation.clone())
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        ControlPayloadSnapshotSession::new(
            registry,
            self.installation.clone(),
            export,
            self.state_root.clone(),
            self.owned_roots.clone(),
            maintenance,
        )
    }

    async fn verify_export(&self, bytes: Vec<u8>) -> UseResult<VerifiedControlStoreExport> {
        self.executor
            .verify_export(bytes, self.installation.clone())
            .await
    }

    async fn register_operation(
        &self,
        reviewed: ReviewedControlOperation,
    ) -> UseResult<ControlOperationRecord> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        self.register_operation_under_maintenance(reviewed).await
    }

    /// Register an operation while the caller retains the installation-wide
    /// shared maintenance guard. The SQLite transaction still ends before
    /// any external payload or provider I/O begins.
    async fn register_operation_under_maintenance(
        &self,
        reviewed: ReviewedControlOperation,
    ) -> UseResult<ControlOperationRecord> {
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let record = self
            .executor
            .register_operation(database_path, self.installation.clone(), reviewed)
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(record)
    }

    async fn cancel_operation(
        &self,
        operation_id: &str,
        plan_digest: &str,
        result_digest: &str,
        cancelled_at_ms: u64,
    ) -> UseResult<ControlOperationRecord> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let record = self
            .executor
            .cancel_operation(
                database_path,
                self.installation.clone(),
                operation_id.to_string(),
                plan_digest.to_string(),
                result_digest.to_string(),
                cancelled_at_ms,
            )
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(record)
    }

    async fn commit_transition(
        &self,
        transition: ControlTransition,
    ) -> UseResult<ControlGeneration> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        self.commit_transition_under_maintenance(transition).await
    }

    /// Commit a transition while the caller retains the installation-wide
    /// shared maintenance guard. This is kept private to the composition
    /// boundary so Runtime plan payload publication can be ordered before the
    /// same transition without opening a second restore window.
    async fn commit_transition_under_maintenance(
        &self,
        transition: ControlTransition,
    ) -> UseResult<ControlGeneration> {
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let generation = self
            .executor
            .commit_transition(database_path, self.installation.clone(), transition)
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(generation)
    }

    /// Project one reviewed operation while the caller retains the shared
    /// installation maintenance guard. The executor performs all reads in a
    /// single SQLite snapshot and returns both the immutable reviewed
    /// operation and its derived transition for the external-payload commit
    /// coordinator.
    async fn project_transition_under_maintenance(
        &self,
        operation_id: &str,
        committed_at_ms: u64,
    ) -> UseResult<(ReviewedControlOperation, ControlTransition)> {
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let projected = self
            .executor
            .project_transition(
                database_path,
                self.installation.clone(),
                operation_id.to_string(),
                committed_at_ms,
            )
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(projected)
    }

    async fn operation(&self, operation_id: &str) -> UseResult<Option<ControlOperationRecord>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        self.executor
            .operation(
                database_path,
                self.installation.clone(),
                operation_id.to_string(),
            )
            .await
    }

    async fn current_generation(&self) -> UseResult<Option<ControlGeneration>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        self.executor
            .current_generation(database_path, self.installation.clone())
            .await
    }

    async fn published_capability(&self) -> UseResult<Option<ControlPublishedCapabilityCursor>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        self.executor
            .published_capability(database_path, self.installation.clone())
            .await
    }

    async fn effects(&self, operation_id: &str) -> UseResult<Vec<ControlEffectRecord>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        self.executor
            .effects(
                database_path,
                self.installation.clone(),
                operation_id.to_string(),
            )
            .await
    }

    async fn claim_next_effect(
        &self,
        claim: ControlEffectClaim,
    ) -> UseResult<Option<ClaimedControlEffect>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let effect = self
            .executor
            .claim_next_effect(database_path, self.installation.clone(), claim)
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(effect)
    }

    async fn record_effect_observation(
        &self,
        observation: ControlEffectObservation,
    ) -> UseResult<bool> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let changed = self
            .executor
            .record_effect_observation(database_path, self.installation.clone(), observation)
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(changed)
    }

    async fn complete_operation(
        &self,
        operation_id: &str,
        plan_digest: &str,
        result_digest: &str,
        completed_at_ms: u64,
    ) -> UseResult<ControlOperationRecord> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        filesystem::require_initialized(&self.state_root, &self.database_path).await?;
        let database_path =
            filesystem::physical_database_path(&self.state_root, &self.database_path).await?;
        let record = self
            .executor
            .complete_operation(
                database_path,
                self.installation.clone(),
                operation_id.to_string(),
                plan_digest.to_string(),
                result_digest.to_string(),
                completed_at_ms,
            )
            .await?;
        filesystem::validate_initialized(&self.state_root, &self.database_path).await?;
        Ok(record)
    }

    async fn restore(&self, bytes: Vec<u8>) -> UseResult<ControlStoreMetadata> {
        let verified = self.verify_export(bytes).await?;
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_exclusive()
            .await?;
        let staging_path =
            filesystem::prepare_clean_restore(&self.state_root, &self.database_path).await?;
        let restore = self
            .executor
            .restore(staging_path, self.installation.clone(), verified.export)
            .await;
        let metadata = match restore {
            Ok(metadata) => metadata,
            Err(error) => {
                if let Err(cleanup) = filesystem::remove_failed_restore(&self.state_root).await {
                    return Err(UseError::new(
                        "use.control_store.restore_cleanup_failed",
                        format!(
                            "Control Store restore failed and staging cleanup also failed: {}; {}",
                            error.message, cleanup.message
                        ),
                    ));
                }
                return Err(error);
            }
        };
        filesystem::activate_clean_restore(&self.state_root, &self.database_path).await?;
        Ok(metadata)
    }

    #[cfg(test)]
    fn database_path(&self) -> &std::path::Path {
        &self.database_path
    }
}

#[cfg(test)]
mod aggregate_tests;
#[cfg(test)]
mod capability_plane_effect_tests;
#[cfg(test)]
mod composition_tests;
#[cfg(test)]
mod cutover_manifest_tests;
#[cfg(test)]
mod dispatcher_tests;
#[cfg(test)]
mod flow_effect_tests;
#[cfg(test)]
mod knowledge_dispatch_tests;
#[cfg(test)]
mod knowledge_effect_test_support;
#[cfg(test)]
mod knowledge_effect_tests;
#[cfg(test)]
mod operation_admission_tests;
#[cfg(test)]
mod payload_control_restore_activation_security_tests;
#[cfg(test)]
mod payload_control_restore_activation_tests;
#[cfg(test)]
mod payload_host_projection_restore_security_tests;
#[cfg(test)]
mod payload_host_projection_restore_tests;
#[cfg(test)]
mod payload_host_projection_tests;
#[cfg(test)]
mod payload_installation_restore_activation_security_tests;
#[cfg(test)]
mod payload_installation_restore_activation_tests;
#[cfg(test)]
mod payload_installation_restore_retirement_security_tests;
#[cfg(test)]
mod payload_installation_restore_staging_security_tests;
#[cfg(test)]
mod payload_installation_restore_staging_tests;
#[cfg(test)]
mod payload_installation_snapshot_security_tests;
#[cfg(test)]
mod payload_installation_snapshot_tests;
#[cfg(test)]
mod payload_knowledge_restore_security_tests;
#[cfg(test)]
mod payload_knowledge_restore_tests;
#[cfg(test)]
mod payload_knowledge_tests;
#[cfg(test)]
mod payload_observation_restore_security_tests;
#[cfg(test)]
mod payload_observation_restore_tests;
#[cfg(test)]
mod payload_observation_security_tests;
#[cfg(test)]
mod payload_observation_tests;
#[cfg(test)]
mod payload_owner_tests;
#[cfg(test)]
mod payload_restore_coordinator_restore_security_tests;
#[cfg(test)]
mod payload_restore_coordinator_restore_tests;
#[cfg(test)]
mod payload_restore_coordinator_security_tests;
#[cfg(test)]
mod payload_restore_coordinator_tests;
#[cfg(test)]
mod payload_runtime_plan_tests;
#[cfg(test)]
mod payload_snapshot_session_tests;
#[cfg(test)]
mod runtime_effect_tests;
#[cfg(test)]
mod static_surface_effect_tests;
#[cfg(test)]
mod tests;
