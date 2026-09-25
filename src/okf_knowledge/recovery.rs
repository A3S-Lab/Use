use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    OkfKnowledgeObservedState, PlanQualifiedSurfaceRef, PlanScope, UseError, UseResult,
};
use a3s_use_extension::{
    ExtensionGenerationLease, ExtensionLifecycleIdentity, ExtensionPaths, ExtensionRegistry,
    InstalledExtension, StateMaintenanceGuard, StateMaintenanceLock, StoredWorkspaceGrant,
};
#[cfg(test)]
use a3s_use_extension::WorkspaceGrantStore;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    OkfKnowledgeBackupManifest, OkfKnowledgeBinding, OkfKnowledgeBindingStore,
    SqliteOkfKnowledgeAdapter,
};
use crate::control_store::{
    control_database_present, ProductionControlHostDependencies, ProductionControlLifecycle,
};
use crate::plugin_lifecycle::{
    PluginLifecycleJournalStore, PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
};

mod diagnostic;
mod filesystem;
mod journal;

pub use diagnostic::{
    OkfKnowledgeRestoreDiagnostic, OkfKnowledgeRestoreOperationDiagnostic,
    OkfKnowledgeRestoreOperationDiagnosticStatus, OKF_KNOWLEDGE_RESTORE_DIAGNOSTIC_SCHEMA,
};
pub use journal::{
    OkfKnowledgeRestoreResult, OKF_KNOWLEDGE_RESTORE_OPERATION_SCHEMA,
    OKF_KNOWLEDGE_RESTORE_RESULT_SCHEMA,
};
use journal::{RestoreOperation, RestoreOperationStatus, RestoreOperationStore};

pub const OKF_KNOWLEDGE_RESTORE_PLAN_SCHEMA: &str = "a3s.use.okf-knowledge-restore-plan.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OkfKnowledgeRestorePlanStatus {
    Required,
    NoChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeFileEvidence {
    pub bytes: u64,
    pub sha256: String,
}

impl OkfKnowledgeFileEvidence {
    fn validate(&self, allow_empty: bool) -> bool {
        (allow_empty || self.bytes > 0)
            && self.bytes <= super::sqlite::MAX_BACKUP_DATABASE_BYTES
            && valid_sha256(&self.sha256)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeDatabaseEvidence {
    pub bytes: u64,
    pub sha256: String,
    pub integrity_verified: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wal: Option<OkfKnowledgeFileEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shm: Option<OkfKnowledgeFileEvidence>,
}

impl OkfKnowledgeDatabaseEvidence {
    fn validate(&self) -> bool {
        OkfKnowledgeFileEvidence {
            bytes: self.bytes,
            sha256: self.sha256.clone(),
        }
        .validate(false)
            && self.wal.as_ref().is_none_or(|value| value.validate(true))
            && self.shm.as_ref().is_none_or(|value| value.validate(true))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeRestorePlan {
    pub schema: String,
    pub scope: PlanScope,
    pub status: OkfKnowledgeRestorePlanStatus,
    pub backup: OkfKnowledgeBackupManifest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_before: Option<OkfKnowledgeDatabaseEvidence>,
    pub authority_digest: String,
    pub binding_state_digest: String,
    pub registry_generation: u64,
    pub retained_projections: usize,
    pub removed_tombstones: usize,
    pub selected_projections: usize,
    pub missing_bindings: usize,
}

impl OkfKnowledgeRestorePlan {
    pub fn validate(&self) -> UseResult<()> {
        self.backup.validate()?;
        let storage = &self.backup.storage;
        let database_matches = matches!(
            &self.database_before,
            Some(current)
                if current.bytes == self.backup.database_bytes
                    && current.sha256 == self.backup.database_sha256
                    && current.integrity_verified
                    && current.wal.is_none()
                    && current.shm.is_none()
        );
        let expected_status = if database_matches && self.missing_bindings == 0 {
            OkfKnowledgeRestorePlanStatus::NoChange
        } else {
            OkfKnowledgeRestorePlanStatus::Required
        };
        if self.schema != OKF_KNOWLEDGE_RESTORE_PLAN_SCHEMA
            || self.scope != self.backup.scope
            || self.status != expected_status
            || !valid_sha256(&self.authority_digest)
            || !valid_sha256(&self.binding_state_digest)
            || self.retained_projections != storage.retained_projections
            || self.removed_tombstones != storage.removed_tombstones
            || self.selected_projections > self.retained_projections
            || self.missing_bindings > self.retained_projections
            || self
                .database_before
                .as_ref()
                .is_some_and(|evidence| !evidence.validate())
        {
            return Err(restore_error(
                "use.okf.knowledge_restore_plan_invalid",
                "The Knowledge restore plan is internally inconsistent or exceeds its evidence bounds.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "encode the Knowledge restore plan")
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }
}

#[derive(Debug, Clone)]
enum KnowledgeGrantAuthority {
    /// Pre-cutover file-store Grants under `grants/` (test fixtures only).
    #[cfg(test)]
    File(WorkspaceGrantStore),
    /// Control-committed Grants (legacy `grants/` must stay absent).
    Control(ExtensionPaths),
}

#[derive(Debug, Clone)]
pub struct OkfKnowledgeRecoveryManager {
    adapter: SqliteOkfKnowledgeAdapter,
    registry: ExtensionRegistry,
    bindings: OkfKnowledgeBindingStore,
    lifecycle: PluginLifecycleJournalStore,
    grants: KnowledgeGrantAuthority,
    maintenance: StateMaintenanceLock,
    operations: RestoreOperationStore,
}

impl OkfKnowledgeRecoveryManager {
    /// Legacy file-store restore path for pre-Control fixtures. Production CLI
    /// and hosts must use [`Self::for_control_authority`].
    #[cfg(test)]
    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self {
            adapter: SqliteOkfKnowledgeAdapter::from_extension_paths(paths),
            registry: ExtensionRegistry::new(paths.clone()),
            bindings: OkfKnowledgeBindingStore::from_extension_paths(paths),
            lifecycle: PluginLifecycleJournalStore::from_extension_paths(paths),
            grants: KnowledgeGrantAuthority::File(WorkspaceGrantStore::from_extension_paths(paths)),
            maintenance: StateMaintenanceLock::new(paths.state_root()),
            operations: RestoreOperationStore::new(paths.installation_state_root()),
        }
    }

    /// Control-authority restore path. Binding and lifecycle roots use
    /// `payloads/*`; Grants are observed from the committed Control generation.
    pub fn for_control_authority(paths: &ExtensionPaths) -> Self {
        Self {
            adapter: SqliteOkfKnowledgeAdapter::from_extension_paths(paths),
            registry: ExtensionRegistry::new(paths.clone()),
            bindings: OkfKnowledgeBindingStore::for_control_authority(paths),
            lifecycle: PluginLifecycleJournalStore::for_control_authority(paths),
            grants: KnowledgeGrantAuthority::Control(paths.clone()),
            maintenance: StateMaintenanceLock::new(paths.state_root()),
            operations: RestoreOperationStore::new(paths.installation_state_root()),
        }
    }

    async fn observe_grant(
        &self,
        scope_id: &str,
        package_id: &str,
        package_digest: &str,
    ) -> UseResult<Option<StoredWorkspaceGrant>> {
        match &self.grants {
            #[cfg(test)]
            KnowledgeGrantAuthority::File(store) => {
                store.observe(scope_id, package_id, package_digest).await
            }
            KnowledgeGrantAuthority::Control(paths) => {
                if !control_database_present(&paths.installation_state_root()) {
                    return Err(restore_error(
                        "use.okf.knowledge_restore_authority_missing",
                        "Control-authority Knowledge restore requires a Control Store database.",
                    ));
                }
                let lifecycle = ProductionControlLifecycle::from_extension_paths(
                    paths,
                    ProductionControlHostDependencies::standalone(
                        paths,
                        std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new()),
                        None,
                    )?,
                )?;
                lifecycle
                    .observe_stored_workspace_grant(scope_id, package_id, package_digest)
                    .await
            }
        }
    }

    /// Build one immutable, path-free restore review from a verified backup
    /// and the exact currently retained package authority. No live state is
    /// changed. Apply re-runs this validation while holding the same
    /// maintenance boundary before publishing any database bytes.
    pub async fn plan_restore(
        &self,
        scope: &PlanScope,
        backup_path: impl Into<PathBuf>,
    ) -> UseResult<OkfKnowledgeRestorePlan> {
        let backup =
            SqliteOkfKnowledgeAdapter::inspect_backup_for_restore(backup_path.into(), scope)
                .await?;
        validate_backup_policy(&backup.manifest, self.adapter.policy())?;
        let _maintenance = self.maintenance.acquire_exclusive().await?;
        self.reject_nonterminal_restore(scope).await?;
        let authority = self.validate_backup_authority(scope, &backup).await?;
        self.build_plan(scope, backup.manifest.clone(), &authority)
            .await
    }

    /// Apply exactly one reviewed restore plan and durably resume the same
    /// operation after interruption.
    pub async fn apply_restore(
        &self,
        scope: &PlanScope,
        backup_path: impl Into<PathBuf>,
        reviewed_plan_digest: &str,
    ) -> UseResult<OkfKnowledgeRestoreResult> {
        if !valid_sha256(reviewed_plan_digest) {
            return Err(restore_error(
                "use.okf.knowledge_restore_plan_mismatch",
                "Knowledge restore requires an exact canonical SHA-256 plan digest.",
            ));
        }

        // Verify before taking the global fence. An interrupted operation can
        // still resume from its exact durable candidate if the external
        // archive is no longer present.
        let inspected =
            match SqliteOkfKnowledgeAdapter::inspect_backup_for_restore(backup_path.into(), scope)
                .await
            {
                Ok(backup) => match validate_backup_policy(&backup.manifest, self.adapter.policy())
                {
                    Ok(()) => Ok(backup),
                    Err(error) => Err(error),
                },
                Err(error) => Err(error),
            };

        let maintenance = self.maintenance.acquire_exclusive().await?;
        let marker = self.operations.active().await?;
        if marker.as_ref().is_some_and(|marker| {
            marker.scope != *scope || marker.plan_digest != reviewed_plan_digest
        }) {
            return Err(restore_in_progress(marker.as_ref()));
        }
        let nonterminal = self.operations.nonterminal(scope).await?;
        if nonterminal
            .as_ref()
            .is_some_and(|operation| operation.plan_digest != reviewed_plan_digest)
        {
            return Err(restore_in_progress_operation(nonterminal.as_ref()));
        }

        let mut existing = self.operations.load(scope, reviewed_plan_digest).await?;
        if existing.is_none() {
            if let Some(marker) = &marker {
                self.operations.begin(&marker.operation).await?;
                existing = Some(marker.operation.clone());
            }
        }

        if let Some(operation) = existing {
            let verified = match inspected {
                Ok(backup) if backup.manifest == operation.plan.backup => Some(backup),
                Ok(_) => {
                    return Err(restore_error(
                        "use.okf.knowledge_restore_backup_mismatch",
                        "The supplied Knowledge backup differs from the backup bound by the reviewed restore operation.",
                    ));
                }
                Err(_) => None,
            };
            return self
                .resume_restore(operation, verified.as_ref(), marker.is_some(), &maintenance)
                .await;
        }

        if let Some(nonterminal) = nonterminal {
            return Err(restore_in_progress_operation(Some(&nonterminal)));
        }
        let backup = inspected?;
        let authority = self.validate_backup_authority(scope, &backup).await?;
        let plan = self
            .build_plan(scope, backup.manifest.clone(), &authority)
            .await?;
        let actual_plan_digest = plan.descriptor_digest()?;
        if actual_plan_digest != reviewed_plan_digest {
            return Err(restore_error(
                "use.okf.knowledge_restore_plan_mismatch",
                "Knowledge state or authority changed after review; create and confirm a new restore plan.",
            )
            .with_detail("actualPlanDigest", serde_json::json!(actual_plan_digest)));
        }
        if plan.status == OkfKnowledgeRestorePlanStatus::NoChange {
            return OkfKnowledgeRestoreResult::no_change(&plan, reviewed_plan_digest.to_owned());
        }

        let database_guard = self.adapter.restore_database_guard(scope).await?;
        let prior_files = filesystem::capture_prior_files(&database_guard).await?;
        let operation = RestoreOperation::new(
            plan,
            reviewed_plan_digest.to_owned(),
            prior_files,
            now_ms()?,
        )?;
        self.operations.prepare(&operation).await?;
        self.operations.activate(&operation).await?;
        maybe_test_crash_marker();
        self.operations.begin(&operation).await?;
        maybe_test_crash(RestoreOperationStatus::Planned);
        let result = self
            .resume_restore_with_guard(operation, Some(&backup), database_guard, &maintenance)
            .await;
        drop(authority);
        result
    }

    async fn build_plan(
        &self,
        scope: &PlanScope,
        manifest: OkfKnowledgeBackupManifest,
        authority: &AuthorityResult,
    ) -> UseResult<OkfKnowledgeRestorePlan> {
        let database_before = self.adapter.database_file_evidence(scope).await?.map(
            |(bytes, sha256, integrity_verified, wal, shm)| OkfKnowledgeDatabaseEvidence {
                bytes,
                sha256,
                integrity_verified,
                wal: wal.map(|(bytes, sha256)| OkfKnowledgeFileEvidence { bytes, sha256 }),
                shm: shm.map(|(bytes, sha256)| OkfKnowledgeFileEvidence { bytes, sha256 }),
            },
        );
        let database_matches = matches!(
            &database_before,
            Some(current)
                if current.bytes == manifest.database_bytes
                    && current.sha256 == manifest.database_sha256
                    && current.integrity_verified
                    && current.wal.is_none()
                    && current.shm.is_none()
        );
        let status = if database_matches && authority.missing_bindings == 0 {
            OkfKnowledgeRestorePlanStatus::NoChange
        } else {
            OkfKnowledgeRestorePlanStatus::Required
        };
        let plan = OkfKnowledgeRestorePlan {
            schema: OKF_KNOWLEDGE_RESTORE_PLAN_SCHEMA.to_owned(),
            scope: scope.clone(),
            status,
            backup: manifest,
            database_before,
            authority_digest: authority.digest.clone(),
            binding_state_digest: authority.binding_state_digest.clone(),
            registry_generation: authority.registry_generation,
            retained_projections: authority.retained_projections,
            removed_tombstones: authority.removed_tombstones,
            selected_projections: authority.selected_projections,
            missing_bindings: authority.missing_bindings,
        };
        plan.validate()?;
        Ok(plan)
    }

    async fn resume_restore(
        &self,
        operation: RestoreOperation,
        verified: Option<&super::sqlite::VerifiedOkfKnowledgeBackup>,
        marker_present: bool,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<OkfKnowledgeRestoreResult> {
        let database_guard = self
            .adapter
            .restore_database_guard(&operation.plan.scope)
            .await?;
        if operation.status == RestoreOperationStatus::Completed {
            let authority = self
                .validate_current_authority(&operation.plan.scope)
                .await?;
            validate_authority_for_plan(&authority, &operation.plan)?;
            let paths = self
                .operations
                .paths(&operation.plan.scope, &operation.plan_digest)?;
            filesystem::validate_published(
                &database_guard,
                &paths,
                &operation.prior_files,
                &operation.plan.backup,
            )
            .await?;
            if marker_present {
                self.operations.clear_active(&operation).await?;
            }
            return operation.result();
        }
        self.operations.activate(&operation).await?;
        maybe_test_crash(operation.status);
        self.resume_restore_with_guard(operation, verified, database_guard, maintenance)
            .await
    }

    async fn resume_restore_with_guard(
        &self,
        mut operation: RestoreOperation,
        verified: Option<&super::sqlite::VerifiedOkfKnowledgeBackup>,
        database_guard: super::sqlite::ScopeDatabaseGuard,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<OkfKnowledgeRestoreResult> {
        let paths = self
            .operations
            .paths(&operation.plan.scope, &operation.plan_digest)?;
        if operation.status == RestoreOperationStatus::Planned {
            filesystem::ensure_candidate(
                &paths,
                verified.map(|backup| backup.database_path.as_path()),
                &operation.plan.backup,
            )
            .await?;
            self.advance_restore(&mut operation, RestoreOperationStatus::Staged, None)
                .await?;
        }
        if operation.status == RestoreOperationStatus::Staged {
            let inventory = self
                .adapter
                .inspect_staged_restore_database(&paths.candidate, &operation.plan.backup)
                .await?;
            let authority = self
                .validate_backup_inventory_authority(
                    &operation.plan.scope,
                    &inventory.bindings,
                    &inventory.selected,
                )
                .await?;
            validate_authority_for_plan(&authority, &operation.plan)?;
            self.bindings
                .restore_exact_inventory(&operation.plan.scope, &inventory.bindings, maintenance)
                .await?;
            if operation.plan.missing_bindings > 0 {
                maybe_test_crash_binding_restore();
            }
            let recovered_authority = self
                .validate_current_authority(&operation.plan.scope)
                .await?;
            validate_authority_for_plan(&recovered_authority, &operation.plan)?;
            self.advance_restore(
                &mut operation,
                RestoreOperationStatus::BindingsRestored,
                None,
            )
            .await?;
        }
        if matches!(
            operation.status,
            RestoreOperationStatus::BindingsRestored
                | RestoreOperationStatus::PriorMoved
                | RestoreOperationStatus::Published
        ) {
            let authority = self
                .validate_current_authority(&operation.plan.scope)
                .await?;
            validate_authority_for_plan(&authority, &operation.plan)?;
        }
        if operation.status == RestoreOperationStatus::BindingsRestored {
            filesystem::ensure_prior_moved(
                &database_guard,
                &paths,
                &operation.prior_files,
                &operation.plan.backup,
            )
            .await?;
            self.advance_restore(&mut operation, RestoreOperationStatus::PriorMoved, None)
                .await?;
        }
        if operation.status == RestoreOperationStatus::PriorMoved {
            filesystem::ensure_published(
                &database_guard,
                &paths,
                &operation.prior_files,
                &operation.plan.backup,
            )
            .await?;
            self.advance_restore(&mut operation, RestoreOperationStatus::Published, None)
                .await?;
        }
        if operation.status == RestoreOperationStatus::Published {
            filesystem::validate_published(
                &database_guard,
                &paths,
                &operation.prior_files,
                &operation.plan.backup,
            )
            .await?;
            self.advance_restore(
                &mut operation,
                RestoreOperationStatus::Completed,
                Some(now_ms()?),
            )
            .await?;
        }
        if operation.status != RestoreOperationStatus::Completed {
            return Err(restore_error(
                "use.okf.knowledge_restore_operation_invalid",
                "The Knowledge restore did not reach a terminal filesystem state.",
            ));
        }
        self.operations.clear_active(&operation).await?;
        operation.result()
    }

    async fn advance_restore(
        &self,
        operation: &mut RestoreOperation,
        status: RestoreOperationStatus,
        completed_at_ms: Option<u64>,
    ) -> UseResult<()> {
        operation.advance(status, completed_at_ms)?;
        self.operations.save(operation).await?;
        maybe_test_crash(status);
        Ok(())
    }

    async fn reject_nonterminal_restore(&self, scope: &PlanScope) -> UseResult<()> {
        let marker = self.operations.active().await?;
        if marker.is_some() {
            return Err(restore_in_progress(marker.as_ref()));
        }
        let operation = self.operations.nonterminal(scope).await?;
        if operation.is_some() {
            return Err(restore_in_progress_operation(operation.as_ref()));
        }
        Ok(())
    }

    async fn validate_backup_authority(
        &self,
        scope: &PlanScope,
        backup: &super::sqlite::VerifiedOkfKnowledgeBackup,
    ) -> UseResult<AuthorityResult> {
        if backup.manifest.scope != *scope || !backup.database_path.is_file() {
            return Err(restore_error(
                "use.okf.knowledge_restore_backup_invalid",
                "The verified Knowledge backup no longer binds its staged database and exact scope.",
            ));
        }
        let removed_tombstones = backup
            .bindings
            .iter()
            .filter(|binding| binding.observation.state == OkfKnowledgeObservedState::Removed)
            .count();
        if backup.manifest.storage.retained_projections != backup.bindings.len()
            || backup.manifest.storage.removed_tombstones != removed_tombstones
        {
            return Err(restore_error(
                "use.okf.knowledge_restore_backup_invalid",
                "The Knowledge backup inventory does not match its retained storage evidence.",
            ));
        }
        self.validate_backup_inventory_authority(scope, &backup.bindings, &backup.selected)
            .await
    }

    async fn validate_backup_inventory_authority(
        &self,
        scope: &PlanScope,
        bindings: &[OkfKnowledgeBinding],
        selected: &[(PlanQualifiedSurfaceRef, u64)],
    ) -> UseResult<AuthorityResult> {
        let current_bindings = self.bindings.list_scope(scope).await?;
        let missing_bindings = validate_current_binding_subset(&current_bindings, bindings)?;
        let binding_state_digest = binding_state_digest(&current_bindings)?;
        self.validate_authority_inventory(
            scope,
            bindings,
            selected,
            binding_state_digest,
            missing_bindings,
        )
        .await
    }

    async fn validate_current_authority(&self, scope: &PlanScope) -> UseResult<AuthorityResult> {
        let bindings = self.bindings.list_scope(scope).await?;
        let selected = selected_from_inventory(&bindings)?;
        let binding_state_digest = binding_state_digest(&bindings)?;
        self.validate_authority_inventory(scope, &bindings, &selected, binding_state_digest, 0)
            .await
    }

    async fn validate_authority_inventory(
        &self,
        scope: &PlanScope,
        bindings: &[OkfKnowledgeBinding],
        selected_inventory: &[(PlanQualifiedSurfaceRef, u64)],
        binding_state_digest: String,
        missing_bindings: usize,
    ) -> UseResult<AuthorityResult> {
        if bindings.iter().any(|binding| {
            matches!(
                binding.observation.state,
                OkfKnowledgeObservedState::Staged | OkfKnowledgeObservedState::Failed
            )
        }) {
            return Err(restore_error(
                "use.okf.knowledge_restore_nonterminal",
                "A Knowledge backup containing a staged or failed projection cannot be restored as terminal authority.",
            ));
        }

        let selected = selected_inventory.iter().cloned().collect::<BTreeSet<_>>();
        if selected.len() != selected_inventory.len()
            || selected.iter().cloned().collect::<Vec<_>>() != selected_inventory
        {
            return Err(selection_mismatch());
        }
        validate_inventory_selections(bindings, selected_inventory)?;

        match &self.grants {
            #[cfg(test)]
            KnowledgeGrantAuthority::File(_) => {
                self.validate_authority_inventory_published(
                    scope,
                    bindings,
                    selected_inventory,
                    binding_state_digest,
                    missing_bindings,
                    selected,
                )
                .await
            }
            KnowledgeGrantAuthority::Control(_) => {
                self.validate_authority_inventory_control(
                    scope,
                    bindings,
                    selected_inventory,
                    binding_state_digest,
                    missing_bindings,
                    selected,
                )
                .await
            }
        }
    }

    #[cfg(test)]
    async fn validate_authority_inventory_published(
        &self,
        scope: &PlanScope,
        bindings: &[OkfKnowledgeBinding],
        selected_inventory: &[(PlanQualifiedSurfaceRef, u64)],
        binding_state_digest: String,
        missing_bindings: usize,
        selected: BTreeSet<(PlanQualifiedSurfaceRef, u64)>,
    ) -> UseResult<AuthorityResult> {
        let snapshot_before = self.registry.snapshot().await?;
        if !snapshot_before.pending_cutovers.is_empty() {
            return Err(restore_error(
                "use.okf.knowledge_restore_registry_busy",
                "Knowledge restore requires a Registry with no pending capability cutover.",
            ));
        }

        let mut packages = BTreeMap::<String, PackageAuthority>::new();
        let mut generation_leases = Vec::new();
        for binding in bindings {
            if binding.observation.state == OkfKnowledgeObservedState::Removed {
                continue;
            }
            let receipt = &binding.receipt;
            let identity = ExtensionLifecycleIdentity::new(
                &receipt.surface.package_id,
                &receipt.package_digest,
                &receipt.manifest_digest,
                receipt.generation,
            )?;
            let selected_projection =
                selected.contains(&(receipt.surface.clone(), receipt.generation));
            let installed = if selected_projection {
                let lease = self
                    .registry
                    .acquire_published_lifecycle_generation(&identity)
                    .await?
                    .ok_or_else(|| {
                        restore_error(
                            "use.okf.knowledge_restore_registry_mismatch",
                            "A selected Knowledge projection is not backed by its exact published package generation.",
                        )
                    })?;
                validate_installed_binding(lease.extension(), binding)?;
                let installed = lease.extension().clone();
                generation_leases.push(lease);
                installed
            } else {
                let installed = self
                    .registry
                    .get_lifecycle_generation(&identity)
                    .await?
                    .ok_or_else(|| {
                        restore_error(
                            "use.okf.knowledge_restore_registry_mismatch",
                            "A retained Knowledge projection has no exact immutable package generation.",
                        )
                    })?;
                validate_installed_binding(&installed, binding)?;
                installed
            };
            let key = format!(
                "{}\n{}\n{}",
                receipt.surface.package_id, receipt.generation, receipt.package_digest
            );
            let entry = packages.entry(key).or_insert_with(|| PackageAuthority {
                package_id: receipt.surface.package_id.clone(),
                package_digest: receipt.package_digest.clone(),
                installed,
                selected: false,
            });
            entry.selected |= selected_projection;
        }

        self.finish_authority_inventory(
            scope,
            bindings,
            selected_inventory,
            binding_state_digest,
            missing_bindings,
            selected.len(),
            packages,
            generation_leases,
            snapshot_before.generation,
            || async {
                let snapshot_after = self.registry.snapshot().await?;
                if snapshot_after != snapshot_before {
                    return Err(restore_error(
                        "use.okf.knowledge_restore_authority_changed",
                        "Registry authority changed while the Knowledge restore plan was being validated.",
                    ));
                }
                Ok(())
            },
        )
        .await
    }

    async fn validate_authority_inventory_control(
        &self,
        scope: &PlanScope,
        bindings: &[OkfKnowledgeBinding],
        selected_inventory: &[(PlanQualifiedSurfaceRef, u64)],
        binding_state_digest: String,
        missing_bindings: usize,
        selected: BTreeSet<(PlanQualifiedSurfaceRef, u64)>,
    ) -> UseResult<AuthorityResult> {
        let paths = self.registry.paths();
        let installation_before = crate::control_store::read_current_installation_snapshot(
            &paths.installation_state_root(),
            paths.installation(),
        )
        .await?
        .ok_or_else(|| {
            restore_error(
                "use.okf.knowledge_restore_authority_missing",
                "Control-authority Knowledge restore requires a Control installation snapshot.",
            )
        })?;

        let mut packages = BTreeMap::<String, PackageAuthority>::new();
        let mut generation_leases = Vec::new();
        for binding in bindings {
            if binding.observation.state == OkfKnowledgeObservedState::Removed {
                continue;
            }
            let receipt = &binding.receipt;
            let identity = ExtensionLifecycleIdentity::new(
                &receipt.surface.package_id,
                &receipt.package_digest,
                &receipt.manifest_digest,
                receipt.generation,
            )?;
            let selected_projection =
                selected.contains(&(receipt.surface.clone(), receipt.generation));
            let selection =
                super::lease::selection_for_identity(&installation_before, &identity).ok_or_else(
                    || {
                        restore_error(
                            "use.okf.knowledge_restore_registry_mismatch",
                            "A Knowledge projection is not backed by its exact Control-selected package generation.",
                        )
                    },
                )?;
            let installed = if selected_projection {
                if !selection.enabled {
                    return Err(restore_error(
                        "use.okf.knowledge_restore_registry_mismatch",
                        "A selected Knowledge projection is disabled in the Control installation snapshot.",
                    ));
                }
                let lease = self
                    .registry
                    .acquire_control_lifecycle_generation(selection, &identity)
                    .await?
                    .ok_or_else(|| {
                        restore_error(
                            "use.okf.knowledge_restore_registry_mismatch",
                            "A selected Knowledge projection could not be leased from Control.",
                        )
                    })?;
                validate_installed_binding(lease.extension(), binding)?;
                let installed = lease.extension().clone();
                generation_leases.push(lease);
                installed
            } else {
                let installed = self
                    .registry
                    .load_control_package_selection(selection)
                    .await
                    .map_err(|_| {
                        restore_error(
                            "use.okf.knowledge_restore_registry_mismatch",
                            "A retained Knowledge projection has no exact Control package generation.",
                        )
                    })?;
                validate_installed_binding(&installed, binding)?;
                installed
            };
            let key = format!(
                "{}\n{}\n{}",
                receipt.surface.package_id, receipt.generation, receipt.package_digest
            );
            let entry = packages.entry(key).or_insert_with(|| PackageAuthority {
                package_id: receipt.surface.package_id.clone(),
                package_digest: receipt.package_digest.clone(),
                installed,
                selected: false,
            });
            entry.selected |= selected_projection;
        }

        let installation_generation = installation_before.generation;
        self.finish_authority_inventory(
            scope,
            bindings,
            selected_inventory,
            binding_state_digest,
            missing_bindings,
            selected.len(),
            packages,
            generation_leases,
            installation_generation,
            || async {
                let installation_after = crate::control_store::read_current_installation_snapshot(
                    &paths.installation_state_root(),
                    paths.installation(),
                )
                .await?;
                if installation_after.as_ref() != Some(&installation_before) {
                    return Err(restore_error(
                        "use.okf.knowledge_restore_authority_changed",
                        "Control installation authority changed while the Knowledge restore plan was being validated.",
                    ));
                }
                Ok(())
            },
        )
        .await
    }

    async fn finish_authority_inventory<F, Fut>(
        &self,
        scope: &PlanScope,
        bindings: &[OkfKnowledgeBinding],
        selected_inventory: &[(PlanQualifiedSurfaceRef, u64)],
        binding_state_digest: String,
        missing_bindings: usize,
        selected_projections: usize,
        packages: BTreeMap<String, PackageAuthority>,
        generation_leases: Vec<ExtensionGenerationLease>,
        registry_generation: u64,
        confirm_stable: F,
    ) -> UseResult<AuthorityResult>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = UseResult<()>>,
    {
        let mut lifecycle_records = Vec::new();
        for package_id in bindings
            .iter()
            .map(|binding| binding.receipt.surface.package_id.clone())
            .collect::<BTreeSet<_>>()
        {
            let record = self
                .lifecycle
                .load_active(scope, &package_id)
                .await?
                .ok_or_else(|| {
                    restore_error(
                        "use.okf.knowledge_restore_authority_missing",
                        "A Knowledge restore package has no durable lifecycle authority.",
                    )
                })?;
            if !matches!(
                record.status,
                PluginLifecycleOperationStatus::Completed
                    | PluginLifecycleOperationStatus::RolledBack
            ) {
                return Err(restore_error(
                    "use.okf.knowledge_restore_lifecycle_active",
                    "Knowledge restore cannot run while a bound package lifecycle is applying or rolling back.",
                ));
            }
            lifecycle_records.push(record);
        }

        let now_ms = now_ms()?;
        let mut grant_records = Vec::new();
        let mut package_receipt_digests = Vec::new();
        for package in packages.values() {
            package_receipt_digests.push(package.installed.receipt.descriptor_digest()?);
            let Ok(catalog) = package.installed.plan_ready_catalog() else {
                continue;
            };
            let ceiling = &catalog.record.permission_ceiling;
            if ceiling.surfaces.is_empty() {
                continue;
            }
            let grant = self
                .observe_grant(scope.id.as_str(), &package.package_id, &package.package_digest)
                .await?
                .ok_or_else(|| {
                    restore_error(
                        "use.okf.knowledge_restore_grant_mismatch",
                        "A permission-bearing Knowledge package has no exact retained Grant authority.",
                    )
                })?;
            match (&grant, package.selected) {
                (StoredWorkspaceGrant::Granted(receipt), true) => {
                    receipt.grant.validate_active_against(ceiling, now_ms)?;
                }
                (StoredWorkspaceGrant::Revoked(_), false) => {}
                _ => {
                    return Err(restore_error(
                        "use.okf.knowledge_restore_grant_mismatch",
                        "The exact Knowledge package Grant does not match its published or retired state.",
                    ));
                }
            }
            grant_records.push(grant);
        }
        package_receipt_digests.sort();

        confirm_stable().await?;

        let evidence = AuthorityEvidence {
            scope,
            bindings,
            selected: selected_inventory,
            lifecycle_records: &lifecycle_records,
            grants: &grant_records,
            package_receipt_digests: &package_receipt_digests,
            registry_generation,
        };
        let digest = format!(
            "sha256:{:x}",
            Sha256::digest(canonical_json(
                &evidence,
                "encode Knowledge restore authority"
            )?)
        );
        Ok(AuthorityResult {
            digest,
            binding_state_digest,
            registry_generation,
            retained_projections: bindings.len(),
            removed_tombstones: bindings
                .iter()
                .filter(|binding| binding.observation.state == OkfKnowledgeObservedState::Removed)
                .count(),
            selected_projections,
            missing_bindings,
            _generation_leases: generation_leases,
        })
    }
}

#[derive(Debug)]
struct PackageAuthority {
    package_id: String,
    package_digest: String,
    installed: InstalledExtension,
    selected: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuthorityEvidence<'a> {
    scope: &'a PlanScope,
    bindings: &'a [OkfKnowledgeBinding],
    selected: &'a [(PlanQualifiedSurfaceRef, u64)],
    lifecycle_records: &'a [PluginLifecycleOperationRecord],
    grants: &'a [StoredWorkspaceGrant],
    package_receipt_digests: &'a [String],
    registry_generation: u64,
}

struct AuthorityResult {
    digest: String,
    binding_state_digest: String,
    registry_generation: u64,
    retained_projections: usize,
    removed_tombstones: usize,
    selected_projections: usize,
    missing_bindings: usize,
    _generation_leases: Vec<ExtensionGenerationLease>,
}

include!("recovery_validate.rs");

#[cfg(test)]
mod tests;
