use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    OkfKnowledgeObservedState, PlanQualifiedSurfaceRef, PlanScope, UseError, UseResult,
};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionPaths, ExtensionRegistry, ExtensionRouteLease,
    InstalledExtension, StoredWorkspaceGrant, WorkspaceGrantStore,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    OkfKnowledgeBackupManifest, OkfKnowledgeBinding, OkfKnowledgeBindingStore,
    SqliteOkfKnowledgeAdapter,
};
use crate::plugin_lifecycle::{
    PluginLifecycleJournalStore, PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};

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
pub struct OkfKnowledgeRecoveryManager {
    adapter: SqliteOkfKnowledgeAdapter,
    registry: ExtensionRegistry,
    bindings: OkfKnowledgeBindingStore,
    lifecycle: PluginLifecycleJournalStore,
    grants: WorkspaceGrantStore,
    maintenance: StateMaintenanceLock,
    operations: RestoreOperationStore,
}

impl OkfKnowledgeRecoveryManager {
    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self {
            adapter: SqliteOkfKnowledgeAdapter::from_extension_paths(paths),
            registry: ExtensionRegistry::new(paths.clone()),
            bindings: OkfKnowledgeBindingStore::from_extension_paths(paths),
            lifecycle: PluginLifecycleJournalStore::from_extension_paths(paths),
            grants: WorkspaceGrantStore::from_extension_paths(paths),
            maintenance: StateMaintenanceLock::new(paths.state_root()),
            operations: RestoreOperationStore::new(paths.state_root()),
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

        let snapshot_before = self.registry.snapshot().await?;
        if !snapshot_before.pending_cutovers.is_empty() {
            return Err(restore_error(
                "use.okf.knowledge_restore_registry_busy",
                "Knowledge restore requires a Registry with no pending capability cutover.",
            ));
        }

        let mut packages = BTreeMap::<String, PackageAuthority>::new();
        let mut route_leases = Vec::new();
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
                route_leases.push(lease);
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
                .grants
                .observe(scope.id.as_str(), &package.package_id, &package.package_digest)
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

        let snapshot_after = self.registry.snapshot().await?;
        if snapshot_after != snapshot_before {
            return Err(restore_error(
                "use.okf.knowledge_restore_authority_changed",
                "Registry authority changed while the Knowledge restore plan was being validated.",
            ));
        }

        let evidence = AuthorityEvidence {
            scope,
            bindings,
            selected: selected_inventory,
            lifecycle_records: &lifecycle_records,
            grants: &grant_records,
            package_receipt_digests: &package_receipt_digests,
            registry_generation: snapshot_before.generation,
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
            registry_generation: snapshot_before.generation,
            retained_projections: bindings.len(),
            removed_tombstones: bindings
                .iter()
                .filter(|binding| binding.observation.state == OkfKnowledgeObservedState::Removed)
                .count(),
            selected_projections: selected.len(),
            missing_bindings,
            _route_leases: route_leases,
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
    _route_leases: Vec<ExtensionRouteLease>,
}

fn selected_from_inventory(
    bindings: &[OkfKnowledgeBinding],
) -> UseResult<Vec<(PlanQualifiedSurfaceRef, u64)>> {
    let mut selected = Vec::new();
    for surface in bindings
        .iter()
        .map(|binding| binding.receipt.surface.clone())
        .collect::<BTreeSet<_>>()
    {
        let records = bindings
            .iter()
            .filter(|binding| binding.receipt.surface == surface)
            .cloned()
            .collect::<Vec<_>>();
        let snapshot = super::store::snapshot_from_records(&records)?;
        match (snapshot.selected, snapshot.projection) {
            (Some(binding), Some(_)) => {
                selected.push((surface, binding.receipt.generation));
            }
            (None, None) => {}
            _ => return Err(selection_mismatch()),
        }
    }
    Ok(selected)
}

fn validate_authority_for_plan(
    authority: &AuthorityResult,
    plan: &OkfKnowledgeRestorePlan,
) -> UseResult<()> {
    if authority.digest != plan.authority_digest
        || authority.registry_generation != plan.registry_generation
        || authority.retained_projections != plan.retained_projections
        || authority.removed_tombstones != plan.removed_tombstones
        || authority.selected_projections != plan.selected_projections
        || authority.missing_bindings > plan.missing_bindings
        || authority.missing_bindings == plan.missing_bindings
            && authority.binding_state_digest != plan.binding_state_digest
    {
        return Err(restore_error(
            "use.okf.knowledge_restore_authority_changed",
            "Knowledge package, binding, lifecycle, Registry, or Grant authority changed after restore review.",
        ));
    }
    Ok(())
}

fn restore_in_progress(marker: Option<&journal::ActiveRestoreMarker>) -> UseError {
    let mut error = restore_error(
        "use.okf.knowledge_restore_in_progress",
        "Another durable Knowledge restore must reach its exact terminal result before planning or applying a different restore.",
    )
    .with_suggestion(
        "Resume the active restore with its reviewed plan digest; do not remove its maintenance marker or retained files.",
    );
    if let Some(marker) = marker {
        error = error.with_detail("activePlanDigest", serde_json::json!(marker.plan_digest));
    }
    error
}

fn restore_in_progress_operation(operation: Option<&RestoreOperation>) -> UseError {
    let mut error = restore_error(
        "use.okf.knowledge_restore_in_progress",
        "A nonterminal Knowledge restore must be resumed before another restore can start.",
    )
    .with_suggestion("Resume the existing restore with its exact reviewed plan digest.");
    if let Some(operation) = operation {
        error = error.with_detail("activePlanDigest", serde_json::json!(operation.plan_digest));
    }
    error
}

#[cfg(test)]
const RESTORE_CRASH_CHECKPOINT_ENV: &str = "A3S_USE_TEST_OKF_RESTORE_CHECKPOINT";

#[cfg(test)]
fn maybe_test_crash(status: RestoreOperationStatus) {
    let checkpoint = match status {
        RestoreOperationStatus::Planned => "planned",
        RestoreOperationStatus::Staged => "staged",
        RestoreOperationStatus::BindingsRestored => "bindings-restored",
        RestoreOperationStatus::PriorMoved => "prior-moved",
        RestoreOperationStatus::Published => "published",
        RestoreOperationStatus::Completed => "completed",
    };
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok(checkpoint) {
        std::process::exit(86);
    }
}

#[cfg(test)]
fn maybe_test_crash_binding_restore() {
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok("binding-file-restored") {
        std::process::exit(86);
    }
}

#[cfg(test)]
fn maybe_test_crash_marker() {
    if std::env::var(RESTORE_CRASH_CHECKPOINT_ENV).as_deref() == Ok("marker-active") {
        std::process::exit(86);
    }
}

#[cfg(not(test))]
fn maybe_test_crash(_status: RestoreOperationStatus) {}

#[cfg(not(test))]
fn maybe_test_crash_marker() {}

#[cfg(not(test))]
fn maybe_test_crash_binding_restore() {}

fn validate_inventory_selections(
    bindings: &[OkfKnowledgeBinding],
    selected: &[(PlanQualifiedSurfaceRef, u64)],
) -> UseResult<()> {
    if selected_from_inventory(bindings)? != selected {
        return Err(selection_mismatch());
    }
    Ok(())
}

fn validate_current_binding_subset(
    current: &[OkfKnowledgeBinding],
    expected: &[OkfKnowledgeBinding],
) -> UseResult<usize> {
    let expected_by_key = expected
        .iter()
        .map(|binding| {
            (
                (binding.receipt.surface.clone(), binding.receipt.generation),
                binding,
            )
        })
        .collect::<BTreeMap<_, _>>();
    if expected_by_key.len() != expected.len() {
        return Err(restore_error(
            "use.okf.knowledge_restore_backup_invalid",
            "The Knowledge backup contains duplicate binding identities.",
        ));
    }
    let mut retained = BTreeSet::new();
    for binding in current {
        let key = (binding.receipt.surface.clone(), binding.receipt.generation);
        if expected_by_key.get(&key).copied() != Some(binding) || !retained.insert(key) {
            return Err(restore_error(
                "use.okf.knowledge_restore_binding_conflict",
                "The current Knowledge binding inventory contains changed or newer evidence outside the reviewed backup.",
            )
            .with_suggestion(
                "Preserve the current state and restore from a coordinated backup that contains this exact binding inventory.",
            ));
        }
    }
    Ok(expected.len().saturating_sub(current.len()))
}

fn binding_state_digest(bindings: &[OkfKnowledgeBinding]) -> UseResult<String> {
    Ok(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json(
            bindings,
            "encode the current Knowledge binding inventory"
        )?)
    ))
}

fn validate_installed_binding(
    installed: &InstalledExtension,
    binding: &OkfKnowledgeBinding,
) -> UseResult<()> {
    let receipt = &binding.receipt;
    if installed.receipt.package_id != receipt.surface.package_id
        || installed.receipt.lifecycle_generation != Some(receipt.generation)
        || installed.receipt.package_sha256.as_deref()
            != receipt.package_digest.strip_prefix("sha256:")
        || installed.receipt.manifest_sha256
            != receipt
                .manifest_digest
                .strip_prefix("sha256:")
                .unwrap_or_default()
        || installed
            .manifest
            .okf
            .iter()
            .find(|surface| surface.id == receipt.surface.surface.id)
            .is_none_or(|surface| surface.bundle != receipt.bundle)
    {
        return Err(restore_error(
            "use.okf.knowledge_restore_registry_mismatch",
            "A Knowledge restore binding does not match its exact immutable package and OKF surface.",
        ));
    }
    Ok(())
}

fn validate_backup_policy(
    manifest: &OkfKnowledgeBackupManifest,
    policy: &super::OkfKnowledgeStoragePolicy,
) -> UseResult<()> {
    let storage = &manifest.storage;
    if storage.max_scope_expanded_bytes != policy.max_scope_expanded_bytes()
        || storage.max_scope_projections != policy.max_scope_projections()
        || storage.max_surface_generations != policy.max_surface_generations()
        || storage.max_scope_tombstones != policy.max_scope_tombstones()
    {
        return Err(restore_error(
            "use.okf.knowledge_restore_policy_mismatch",
            "The Knowledge backup storage policy differs from the current host policy.",
        ));
    }
    Ok(())
}

fn selection_mismatch() -> UseError {
    restore_error(
        "use.okf.knowledge_restore_selection_mismatch",
        "The backup selection differs from the exact durable Knowledge binding projection.",
    )
}

fn canonical_json(value: &(impl Serialize + ?Sized), action: &str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        restore_error(
            "use.okf.knowledge_restore_plan_invalid",
            format!("Failed to {action}: {error}"),
        )
    })?;
    Ok(bytes)
}

fn now_ms() -> UseResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            restore_error(
                "use.okf.knowledge_restore_clock_invalid",
                format!("The system clock is before the Unix epoch: {error}"),
            )
        })?
        .as_millis();
    u64::try_from(millis)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| {
            restore_error(
                "use.okf.knowledge_restore_clock_invalid",
                "The system clock exceeds the Knowledge restore timestamp range.",
            )
        })
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn restore_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests;
