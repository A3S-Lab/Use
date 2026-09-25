use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use a3s_use_core::{PluginOperationAction, PluginPackageLock, UseResult};

use super::plan::{now_ms, package_state_revision, uninstall_operation};
use super::store::PendingPackageGraphOperation;
use super::{
    package_manager_error, CognitivePackageManager, CognitivePackageUninstallResult,
    UninstallDisposition,
};

impl CognitivePackageManager {
    pub(super) async fn uninstall_plan_lock(
        &self,
        root_package_id: &str,
    ) -> UseResult<Option<PluginPackageLock>> {
        self.installed_package_lock(root_package_id).await
    }

    pub async fn owns_installed_root(&self, root_package_id: &str) -> UseResult<bool> {
        Ok(self
            .installed_package_lock(root_package_id)
            .await?
            .is_some())
    }

    /// Remove one installed root and dependency nodes no longer referenced by
    /// any other installed root. Removal follows the exact reverse lock order
    /// and resumes from pending manifest/generation evidence after a crash.
    pub async fn uninstall(
        &self,
        root_package_id: &str,
    ) -> UseResult<CognitivePackageUninstallResult> {
        let maintenance = Arc::new(self.maintenance_lock().acquire_shared().await?);
        let _mutation = self.installation_mutation_lock().acquire().await?;
        self.require_graph_mutation_domain(PluginOperationAction::Uninstall, root_package_id)
            .await?;
        self.uninstall_through_control(root_package_id, maintenance)
            .await
    }

    async fn uninstall_through_control(
        &self,
        root_package_id: &str,
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<CognitivePackageUninstallResult> {
        let lock = self
            .installed_package_lock(root_package_id)
            .await?
            .ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_missing",
                    format!(
                        "Cognitive package '{root_package_id}' has no Control installation ownership record."
                    ),
                )
            })?;
        let lock_digest = lock.descriptor_digest()?;
        let control = self.ensure_control().await?;
        let snapshot = control.current_snapshot().await?.ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_missing",
                "Control Store has no committed installation snapshot for uninstall.",
            )
        })?;
        let installed_locks = self.installed_package_locks().await?;
        let dispositions = self
            .uninstall_dispositions_control(&lock, &installed_locks)
            .await?;
        let artifact_store = self.registry.paths().artifact_store();
        let mut generations = BTreeMap::new();
        let mut manifests = BTreeMap::new();
        let mut surface_selections = BTreeMap::new();
        for (package_id, disposition) in &dispositions {
            let selection = snapshot.package_selection(package_id).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "An uninstall package is absent from the Control snapshot.",
                )
            })?;
            surface_selections.insert(package_id.clone(), selection.selected_surfaces.clone());
            if *disposition != UninstallDisposition::Remove {
                continue;
            }
            generations.insert(package_id.clone(), selection.state_generation);
            let verified = artifact_store
                .acquire_verified_package(&selection.package.catalog)
                .await?;
            manifests.insert(package_id.clone(), verified.manifest().clone());
        }
        for manifest in manifests.values() {
            self.lifecycle.validate_manifest_for_retirement(manifest)?;
        }
        let root_selection = snapshot.package_selection(root_package_id).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "The uninstall root disappeared from the Control snapshot.",
            )
        })?;
        let root_receipt_digest = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(b"a3s.use.control.uninstall-root.v1\0");
            hasher.update(root_package_id.as_bytes());
            hasher.update(b"\0");
            hasher.update(root_selection.state_generation.to_le_bytes());
            format!("sha256:{:x}", hasher.finalize())
        };
        let capability_generation = snapshot.generation;
        let grant_snapshot = self
            .planned_grant_snapshot(package_state_revision(capability_generation)?)
            .await?;
        let generated = uninstall_operation(
            &lock,
            &dispositions,
            &surface_selections,
            generations,
            root_receipt_digest,
            capability_generation,
            self.scope(),
            now_ms()?,
            &grant_snapshot,
            self.authorization.as_ref(),
        )?;
        let planned_at_ms = generated.envelope.plan.created_at_ms;
        let pending = PendingPackageGraphOperation::planned(
            generated.envelope,
            planned_at_ms,
            generated.generations,
            manifests,
        )?;
        let pending = self
            .admit_planned_graph_operation_in_memory(pending)
            .await?;
        self.authorization.verify_plan(&pending.envelope)?;
        let publications = self.lifecycle.runtime_plan_publications()?;
        super::control_authority::require_control_runtime_readiness_for_publications(
            self.lifecycle.control_runtime_readiness().as_ref(),
            &publications,
        )?;
        let _snapshot = super::control_authority::apply_pending_through_control(
            control,
            &pending,
            &publications,
            maintenance,
        )
        .await?;
        let removed_packages = lock
            .removal_order()?
            .into_iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&UninstallDisposition::Remove)
            })
            .map(|package| package.package_id().to_string())
            .collect();
        let retained_packages = lock
            .install_order()?
            .into_iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&UninstallDisposition::Retain)
            })
            .map(|package| package.package_id().to_string())
            .collect();
        Ok(CognitivePackageUninstallResult {
            changed: true,
            root_package_id: root_package_id.to_string(),
            package_lock: lock,
            package_lock_digest: lock_digest,
            plan: pending.envelope,
            removed_packages,
            retained_packages,
        })
    }

    async fn uninstall_dispositions_control(
        &self,
        lock: &PluginPackageLock,
        installed_locks: &[PluginPackageLock],
    ) -> UseResult<BTreeMap<String, UninstallDisposition>> {
        let closure = lock
            .packages
            .iter()
            .map(|package| package.package_id().to_string())
            .collect::<BTreeSet<_>>();
        let mut retained = installed_locks
            .iter()
            .filter(|installed_lock| installed_lock.root_package_id != lock.root_package_id)
            .flat_map(|installed_lock| {
                installed_lock
                    .packages
                    .iter()
                    .map(|package| package.package_id().to_string())
            })
            .filter(|package_id| closure.contains(package_id))
            .collect::<BTreeSet<_>>();
        loop {
            let before = retained.len();
            for package_id in retained.clone() {
                if let Some(package) = lock.package(&package_id) {
                    retained.extend(
                        package
                            .dependencies
                            .iter()
                            .map(|dependency| dependency.package_id.clone()),
                    );
                }
            }
            if retained.len() == before {
                break;
            }
        }
        if retained.contains(&lock.root_package_id) {
            return Err(package_manager_error(
                "use.plugin.package_has_dependents",
                format!(
                    "Cognitive package '{}' is still required by another installed package graph.",
                    lock.root_package_id
                ),
            ));
        }
        Ok(lock
            .packages
            .iter()
            .map(|package| {
                let disposition = if retained.contains(package.package_id()) {
                    UninstallDisposition::Retain
                } else {
                    UninstallDisposition::Remove
                };
                (package.package_id().to_string(), disposition)
            })
            .collect())
    }
}
