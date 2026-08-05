use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PlanPackageChangeKind, PluginOperationAction, UseResult};
use a3s_use_extension::{ExtensionLifecycleIdentity, ExtensionManifest, InstalledExtension};

use crate::plugin_lifecycle::{
    ExtensionGraphCapabilityLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginPackageGraphLifecycleCoordinator, PluginPackageLifecycleUnit,
};

use super::grant::authorize_planned_operation;
use super::plan::{now_ms, package_state_revision, uninstall_operation};
use super::store::{InstalledPackageGraph, PendingPackageGraphOperation};
use super::{
    all_catalog_surfaces, installed_matches_lock, package_manager_error, CognitivePackageManager,
    CognitivePackageUninstallResult, UninstallDisposition,
};

impl CognitivePackageManager {
    pub async fn owns_installed_root(&self, root_package_id: &str) -> UseResult<bool> {
        if self.graph_store().get(root_package_id).await?.is_some() {
            return Ok(true);
        }
        Ok(self
            .pending_store()
            .get(PluginOperationAction::Uninstall, root_package_id)
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
        let graph_store = self.graph_store();
        let pending_store = self.pending_store();
        let existing_pending = pending_store
            .get(PluginOperationAction::Uninstall, root_package_id)
            .await?;
        let graph = graph_store.get(root_package_id).await?;
        let (lock, lock_digest) = match (&graph, &existing_pending) {
            (Some(graph), Some(pending)) => {
                validate_pending_lock(pending, &graph.package_lock)?;
                (
                    graph.package_lock.clone(),
                    graph.package_lock_digest.clone(),
                )
            }
            (Some(graph), None) => (
                graph.package_lock.clone(),
                graph.package_lock_digest.clone(),
            ),
            (None, Some(pending)) => {
                pending.validate()?;
                let lock = pending.envelope.package_lock.clone().ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A pending cognitive-package uninstall omitted its exact dependency lock.",
                    )
                })?;
                if lock.root_package_id != root_package_id {
                    return Err(package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A pending cognitive-package uninstall does not own its root package path.",
                    ));
                }
                let lock_digest = lock.descriptor_digest()?;
                (lock, lock_digest)
            }
            (None, None) => {
                return Err(package_manager_error(
                    "use.plugin.package_graph_missing",
                    format!(
                        "Cognitive package '{}' has no installed dependency-lock ownership record.",
                        root_package_id
                    ),
                ))
            }
        };

        let mut installed = self.installed_lock_nodes(&lock).await?;
        let pending = if let Some(pending) = existing_pending {
            validate_pending_lock(&pending, &lock)?;
            pending
        } else {
            let exact = require_fresh_installed_closure(&lock, &installed)?;
            let dispositions = self
                .uninstall_dispositions(&lock, &graph_store.list().await?)
                .await?;
            let root = exact.get(root_package_id).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The uninstall root disappeared from its exact installed closure.",
                )
            })?;
            let mut generations = BTreeMap::new();
            let mut manifests = BTreeMap::new();
            for (package_id, disposition) in &dispositions {
                if *disposition != UninstallDisposition::Remove {
                    continue;
                }
                let extension = exact.get(package_id).ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A removed package is absent from the installed closure.",
                    )
                })?;
                generations.insert(
                    package_id.clone(),
                    extension.receipt.lifecycle_generation.ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_generation_changed",
                            "A lifecycle-managed package omitted its exact generation.",
                        )
                    })?,
                );
                manifests.insert(package_id.clone(), extension.manifest.clone());
            }
            for manifest in manifests.values() {
                self.lifecycle.validate_manifest(manifest)?;
            }
            let snapshot = self.registry.snapshot().await?;
            let grant_snapshot = self
                .grant_store()
                .snapshot_scope(&self.scope_id, package_state_revision(snapshot.generation)?)
                .await?;
            let generated = uninstall_operation(
                &lock,
                &dispositions,
                generations,
                root.receipt.descriptor_digest()?,
                snapshot.generation,
                &self.scope_id,
                now_ms()?,
                &grant_snapshot,
                self.authorization.as_ref(),
            )?;
            let admitted_at_ms = now_ms()?;
            let authorization = authorize_planned_operation(
                self.authorization.as_ref(),
                &generated.envelope,
                generated.grants.as_ref(),
                admitted_at_ms,
            )
            .await?;
            let pending = PendingPackageGraphOperation::new(
                generated.envelope,
                admitted_at_ms,
                authorization,
                generated.generations,
                manifests,
            )?;
            pending_store.put(&pending).await?;
            pending
        };
        if pending.requires_authority_revalidation() {
            self.authorization
                .verify_authority(&pending.envelope.plan)?;
        }
        for manifest in pending.manifests.values() {
            self.lifecycle.validate_manifest(manifest)?;
        }
        let dispositions = pending_dispositions(&pending)?;
        let apply_time = now_ms()?;

        let mut units = Vec::with_capacity(pending.generations.len());
        for package in lock.removal_order()? {
            if dispositions.get(package.package_id()) != Some(&UninstallDisposition::Remove) {
                continue;
            }
            let manifest = pending.manifests.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A removed package has no pending admitted manifest.",
                )
            })?;
            let generation = *pending
                .generations
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A removed package has no pending lifecycle generation.",
                    )
                })?;
            let state = package
                .catalog
                .selected_state(&all_catalog_surfaces(package))?;
            let identity = ExtensionLifecycleIdentity::new(
                package.package_id(),
                state.release.package_sha256.clone(),
                state.release.manifest_sha256.clone(),
                generation,
            )?;
            let package_root = self.registry.lifecycle_package_root(&identity);
            let intent = PluginLifecycleIntent::from_manifest(
                PluginLifecycleIntentSpec {
                    operation_id: pending.envelope.plan.operation_id.clone(),
                    plan_digest: pending.envelope.plan_digest.clone(),
                    scope_id: self.scope_id.clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: identity.package_digest().to_string(),
                    manifest_digest: identity.manifest_digest().to_string(),
                    generation,
                    action: PluginLifecycleAction::Uninstall,
                },
                manifest,
            )?;
            match installed.remove(package.package_id()).flatten() {
                Some(extension) => validate_installed_unit(&extension, manifest, &identity)?,
                None => self.validate_missing_replay(&intent).await?,
            }
            units.push(PluginPackageLifecycleUnit::new(
                self.lifecycle
                    .uninstall_coordinator(self.registry.clone(), package_root)?,
                intent,
                manifest.clone(),
            )?);
        }

        let coordinator = PluginPackageGraphLifecycleCoordinator::new(std::sync::Arc::new(
            ExtensionGraphCapabilityLifecycleHost::new(self.registry.clone()),
        ));
        match pending
            .authorization
            .lifecycle_unit(self.grant_store(), &pending.envelope)?
        {
            Some(grants) => {
                coordinator
                    .apply_uninstall_with_grants(&pending.envelope, &units, &grants, || {
                        now_ms().unwrap_or(apply_time)
                    })
                    .await?;
            }
            None => {
                coordinator
                    .apply_uninstall(&pending.envelope, &units, || now_ms().unwrap_or(apply_time))
                    .await?;
            }
        }
        graph_store.remove(root_package_id, &lock_digest).await?;
        pending_store.remove(&pending).await?;
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

    async fn installed_lock_nodes(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
    ) -> UseResult<BTreeMap<String, Option<InstalledExtension>>> {
        let mut installed = BTreeMap::new();
        for package in &lock.packages {
            let extension = self.registry.get(package.package_id()).await?;
            if let Some(extension) = &extension {
                if !installed_matches_lock(extension, &package.catalog)? {
                    return Err(package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Installed dependency '{}' no longer matches the exact lock.",
                            package.package_id()
                        ),
                    ));
                }
            }
            installed.insert(package.package_id().to_string(), extension);
        }
        Ok(installed)
    }

    async fn validate_missing_replay(&self, intent: &PluginLifecycleIntent) -> UseResult<()> {
        let journal = crate::plugin_lifecycle::PluginLifecycleJournalStore::from_extension_paths(
            self.registry.paths(),
        );
        let matches = journal
            .load_active(&intent.scope_id, &intent.package_id)
            .await?
            .is_some_and(|record| record.intent == *intent);
        if matches {
            Ok(())
        } else {
            Err(package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                format!(
                    "Missing package '{}' has no exact pending uninstall journal to replay.",
                    intent.package_id
                ),
            ))
        }
    }

    async fn uninstall_dispositions(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
        graphs: &[InstalledPackageGraph],
    ) -> UseResult<BTreeMap<String, UninstallDisposition>> {
        let closure = lock
            .packages
            .iter()
            .map(|package| package.package_id().to_string())
            .collect::<BTreeSet<_>>();
        let mut retained = graphs
            .iter()
            .filter(|graph| graph.package_lock.root_package_id != lock.root_package_id)
            .flat_map(|graph| {
                graph
                    .package_lock
                    .packages
                    .iter()
                    .map(|package| package.package_id().to_string())
            })
            .filter(|package_id| closure.contains(package_id))
            .collect::<BTreeSet<_>>();
        for extension in self.registry.list().await? {
            if closure.contains(&extension.receipt.package_id) {
                continue;
            }
            for dependency in &extension.manifest.dependencies {
                if closure.contains(&dependency.package_id) {
                    retained.insert(dependency.package_id.clone());
                }
            }
        }
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

fn require_fresh_installed_closure(
    lock: &a3s_use_core::PluginPackageLock,
    installed: &BTreeMap<String, Option<InstalledExtension>>,
) -> UseResult<BTreeMap<String, InstalledExtension>> {
    let mut exact = BTreeMap::new();
    for package in &lock.packages {
        let extension = installed
            .get(package.package_id())
            .and_then(Option::as_ref)
            .ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Installed dependency '{}' is missing from root '{}'.",
                        package.package_id(),
                        lock.root_package_id
                    ),
                )
            })?;
        if !extension.receipt.enabled {
            return Err(package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                format!(
                    "Installed dependency '{}' is not in the published capability generation.",
                    package.package_id()
                ),
            ));
        }
        exact.insert(package.package_id().to_string(), extension.clone());
    }
    Ok(exact)
}

fn pending_dispositions(
    pending: &PendingPackageGraphOperation,
) -> UseResult<BTreeMap<String, UninstallDisposition>> {
    pending.validate()?;
    pending
        .envelope
        .plan
        .packages
        .iter()
        .map(|package| {
            let disposition = match package.change {
                PlanPackageChangeKind::Remove => UninstallDisposition::Remove,
                PlanPackageChangeKind::Retain => UninstallDisposition::Retain,
                _ => {
                    return Err(package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A pending uninstall contains an unsupported package transition.",
                    ))
                }
            };
            Ok((package.package_id.clone(), disposition))
        })
        .collect()
}

fn validate_pending_lock(
    pending: &PendingPackageGraphOperation,
    lock: &a3s_use_core::PluginPackageLock,
) -> UseResult<()> {
    pending.validate()?;
    if pending.envelope.plan.action != PluginOperationAction::Uninstall
        || pending.envelope.package_lock.as_ref() != Some(lock)
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_busy",
            "The pending cognitive-package uninstall no longer matches the installed graph.",
        ));
    }
    Ok(())
}

fn validate_installed_unit(
    extension: &InstalledExtension,
    manifest: &ExtensionManifest,
    identity: &ExtensionLifecycleIdentity,
) -> UseResult<()> {
    if extension.manifest != *manifest
        || extension.receipt.lifecycle_generation != Some(identity.generation())
        || extension.receipt.package_sha256.as_deref()
            != identity.package_digest().strip_prefix("sha256:")
        || extension.receipt.manifest_sha256
            != identity
                .manifest_digest()
                .strip_prefix("sha256:")
                .unwrap_or_default()
    {
        return Err(package_manager_error(
            "use.plugin.package_generation_changed",
            format!(
                "Package '{}' changed generation before uninstall apply.",
                extension.receipt.package_id
            ),
        ));
    }
    Ok(())
}
