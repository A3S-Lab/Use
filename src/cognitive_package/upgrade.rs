use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use a3s_use_core::{
    PluginOperationAction, PluginPackageLock, PluginReleaseChannel, PluginSurfaceRef, UseResult,
};
use a3s_use_extension::{
    ExtensionLifecyclePackage, ExtensionManifest, ExtensionRegistrySnapshot, InstalledExtension,
    TrustedRegistry,
};

use super::download_attempt::PendingPackageDownloadAttempt;
use super::install::verify_expected_lock;
use super::plan::{now_ms, package_state_revision, upgrade_operation};
use super::registry_access::{download_selected_packages, resolve_package_lock, RegistryAccess};
use super::resolution_attempt::PendingPackageResolutionAttempt;
use super::store::{PackageGraphOperationPhase, PendingPackageGraphOperation};
use super::upgrade_validation::{pending_upgrade_dispositions, validate_pending_upgrade};
use super::{
    installed_matches_lock, package_manager_error, CognitivePackageManager,
    CognitivePackageUpgradeResult, UpgradeDisposition,
};

struct PreparedUpgradePackage {
    package: ExtensionLifecyclePackage,
    manifest: ExtensionManifest,
}

impl CognitivePackageManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn upgrade_remote_with_access(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
        access: RegistryAccess,
        requested_root_surfaces: Option<&[PluginSurfaceRef]>,
    ) -> UseResult<CognitivePackageUpgradeResult> {
        let maintenance = Arc::new(self.maintenance_lock().acquire_shared().await?);
        let _mutation = self.installation_mutation_lock().acquire().await?;
        self.require_graph_mutation_domain(PluginOperationAction::Upgrade, package_id)
            .await?;
        let mut resolution_attempt = Some(
            self.resolution_attempt_store()
                .begin(PendingPackageResolutionAttempt::new(
                    self.scope().clone(),
                    PluginOperationAction::Upgrade,
                    package_id,
                    requested_version,
                    channel,
                    access.resolution_access(),
                    root_registry,
                    dependency_registries,
                    now_ms()?,
                )?)
                .await?,
        );
        let candidate_lock = match resolve_package_lock(
            access,
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            resolution_attempt.as_ref().ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_resolution_attempt_invalid",
                    "The pre-lock Registry resolution observer is unavailable.",
                )
            })?,
        )
        .await
        {
            Ok(lock) => {
                resolution_attempt
                    .as_ref()
                    .ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_resolution_attempt_invalid",
                            "The pre-lock Registry resolution observer is unavailable.",
                        )
                    })?
                    .mark_resolved(&lock)
                    .await?;
                lock
            }
            Err(error) => {
                resolution_attempt
                    .as_ref()
                    .ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_resolution_attempt_invalid",
                            "The pre-lock Registry resolution observer is unavailable.",
                        )
                    })?
                    .mark_failed(&error.code)
                    .await?;
                return Err(error);
            }
        };
        let candidate_digest = candidate_lock.descriptor_digest()?;
        verify_expected_lock(&candidate_digest, expected_package_lock_digest)?;

        let existing_graph = self.installed_package_lock(package_id).await?;
        let existing_pending = None::<PendingPackageGraphOperation>;
        let prior_lock = match (&existing_pending, &existing_graph) {
            (Some(pending), graph) => {
                validate_pending_upgrade(
                    pending,
                    &candidate_lock,
                    graph.as_ref(),
                    self.scope(),
                )?;
                pending.prior_package_lock.clone().ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A pending upgrade omitted its exact prior dependency lock.",
                    )
                })?
            }
            (None, Some(graph)) => graph.clone(),
            (None, None) => {
                return Err(package_manager_error(
                    "use.plugin.package_graph_missing",
                    format!(
                        "Cognitive package '{package_id}' has no installed dependency-lock ownership record."
                    ),
                ))
            }
        };
        if prior_lock.root_package_id != package_id {
            return Err(package_manager_error(
                "use.plugin.package_graph_invalid",
                "The installed dependency graph does not own the requested upgrade root.",
            ));
        }

        let installed_locks = self.installed_package_locks().await?;
        // Control ownership is the Installation Snapshot; Registry publication
        // is not consulted for disposition planning.
        let installed_extensions = Vec::new();
        let snapshot = ExtensionRegistrySnapshot::empty(self.scope().clone())?;
        let mut dispositions = upgrade_dispositions(
            &prior_lock,
            &candidate_lock,
            &installed_locks,
            &installed_extensions,
            &snapshot,
        )?;
        // Registry is empty under Control; promote shared lock nodes that
        // already match another root's exact catalog to Retain.
        for candidate in &candidate_lock.packages {
            if prior_lock.package(candidate.package_id()).is_some() {
                continue;
            }
            if dispositions.get(candidate.package_id()) != Some(&UpgradeDisposition::Add) {
                continue;
            }
            let shared = installed_locks.iter().any(|lock| {
                lock.root_package_id != prior_lock.root_package_id
                    && lock
                        .package(candidate.package_id())
                        .is_some_and(|package| package.catalog == candidate.catalog)
            });
            if shared {
                dispositions.insert(
                    candidate.package_id().to_string(),
                    UpgradeDisposition::Retain,
                );
            }
        }
        if let Some(requested) = requested_root_surfaces {
            let root = candidate_lock.package(package_id).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The upgrade candidate lock omitted its root package.",
                )
            })?;
            let mut expected = root
                .catalog
                .record
                .resolve_surfaces(requested)?
                .into_iter()
                .map(|surface| surface.reference())
                .collect::<Vec<_>>();
            expected.sort();
            let control = self.ensure_control().await?;
            let snapshot = control.current_snapshot().await?.ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_missing",
                    "Control Store has no installation snapshot for surface selection.",
                )
            })?;
            let current_surfaces = snapshot
                .package_selection(package_id)
                .map(|selection| selection.selected_surfaces.clone())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_missing",
                        "The installed upgrade root disappeared before surface selection.",
                    )
                })?;
            if dispositions.get(package_id) == Some(&UpgradeDisposition::Retain)
                && current_surfaces != expected
            {
                dispositions.insert(package_id.to_string(), UpgradeDisposition::Replace);
            }
        }
        if existing_pending.is_none()
            && dispositions
                .values()
                .all(|disposition| *disposition == UpgradeDisposition::Retain)
        {
            let mut installed = self.require_published_prior(&prior_lock).await?;
            self.add_published_candidate_retentions(
                &prior_lock,
                &candidate_lock,
                &dispositions,
                &mut installed,
            )
            .await?;
            let root = installed.get(package_id).cloned().ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The retained upgrade root disappeared from its installed graph.",
                )
            })?;
            resolution_attempt
                .take()
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_resolution_attempt_invalid",
                        "The pre-lock Registry resolution observer is unavailable.",
                    )
                })?
                .finish()
                .await?;
            return Ok(CognitivePackageUpgradeResult {
                changed: false,
                root,
                prior_package_lock: prior_lock,
                package_lock: candidate_lock,
                package_lock_digest: candidate_digest,
                plan: None,
                added_packages: Vec::new(),
                replaced_packages: Vec::new(),
                removed_packages: Vec::new(),
                retained_packages: dispositions.keys().cloned().collect(),
            });
        }

        let mut registries = Vec::with_capacity(dependency_registries.len() + 1);
        registries.push(root_registry.clone());
        registries.extend(dependency_registries.iter().cloned());
        let selected_downloads: BTreeSet<String> = dispositions
            .iter()
            .filter_map(|(package_id, disposition)| {
                matches!(
                    disposition,
                    UpgradeDisposition::Add | UpgradeDisposition::Replace
                )
                .then_some(package_id.clone())
            })
            .collect();
        let download_store = self.download_attempt_store();
        let mut download_attempt = if selected_downloads.is_empty() {
            None
        } else {
            Some(
                resolution_attempt
                    .take()
                    .ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_resolution_attempt_invalid",
                            "The pre-lock Registry resolution observer is unavailable.",
                        )
                    })?
                    .into_download(
                        &download_store,
                        PendingPackageDownloadAttempt::new(
                            self.scope().clone(),
                            PluginOperationAction::Upgrade,
                            candidate_lock.clone(),
                            selected_downloads.clone(),
                            now_ms()?,
                        )?,
                    )
                    .await?,
            )
        };
        let downloads =
            download_selected_packages(access, &candidate_lock, &registries, &selected_downloads)
                .await?;
        let mut prepared = BTreeMap::new();
        for download in downloads {
            let package_id = download.resolved().package_id.clone();
            if dispositions.get(&package_id) == Some(&UpgradeDisposition::Retain) {
                continue;
            }
            let package = ExtensionLifecyclePackage::prepare_remote(&package_id, download).await?;
            let manifest = package.manifest().clone();
            if prepared
                .insert(package_id, PreparedUpgradePackage { package, manifest })
                .is_some()
            {
                return Err(package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A prepared upgrade package appears more than once.",
                ));
            }
        }
        validate_prepared_candidates(&dispositions, &prepared)?;

        let pending = if let Some(pending) = existing_pending {
            validate_pending_upgrade(
                &pending,
                &candidate_lock,
                existing_graph.as_ref(),
                self.scope(),
            )?;
            for (package_id, prepared) in &prepared {
                if pending.manifests.get(package_id) != Some(&prepared.manifest) {
                    return Err(package_manager_error(
                        "use.plugin.package_changed",
                        format!(
                            "Prepared package '{package_id}' no longer matches its admitted upgrade manifest."
                        ),
                    ));
                }
            }
            pending
        } else {
            let mut installed = self.require_published_prior(&prior_lock).await?;
            self.add_published_candidate_retentions(
                &prior_lock,
                &candidate_lock,
                &dispositions,
                &mut installed,
            )
            .await?;
            let mut manifests = BTreeMap::new();
            let mut prior_generations = BTreeMap::new();
            let mut prior_manifests = BTreeMap::new();
            for candidate in &candidate_lock.packages {
                match dispositions.get(candidate.package_id()) {
                    Some(UpgradeDisposition::Retain) => {
                        let extension = installed.get(candidate.package_id()).ok_or_else(|| {
                            package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A retained upgrade dependency is absent from the installed graph.",
                            )
                        })?;
                        manifests.insert(
                            candidate.package_id().to_string(),
                            extension.manifest.clone(),
                        );
                    }
                    Some(UpgradeDisposition::Replace) => {
                        let extension = installed.get(candidate.package_id()).ok_or_else(|| {
                            package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A replaced upgrade dependency is absent from the installed graph.",
                            )
                        })?;
                        prior_generations.insert(
                            candidate.package_id().to_string(),
                            extension.receipt.lifecycle_generation.ok_or_else(|| {
                                package_manager_error(
                                    "use.plugin.package_generation_changed",
                                    "A prior package omitted its lifecycle generation.",
                                )
                            })?,
                        );
                        prior_manifests.insert(
                            candidate.package_id().to_string(),
                            extension.manifest.clone(),
                        );
                        manifests.insert(
                            candidate.package_id().to_string(),
                            prepared
                                .get(candidate.package_id())
                                .ok_or_else(|| {
                                    package_manager_error(
                                        "use.plugin.package_graph_invalid",
                                        "A replacement package was not prepared.",
                                    )
                                })?
                                .manifest
                                .clone(),
                        );
                    }
                    Some(UpgradeDisposition::Add) => {
                        manifests.insert(
                            candidate.package_id().to_string(),
                            prepared
                                .get(candidate.package_id())
                                .ok_or_else(|| {
                                    package_manager_error(
                                        "use.plugin.package_graph_invalid",
                                        "An added package was not prepared.",
                                    )
                                })?
                                .manifest
                                .clone(),
                        );
                    }
                    Some(UpgradeDisposition::Remove) => {
                        return Err(package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "A removed package appeared in the candidate dependency lock.",
                        ))
                    }
                    None => {
                        return Err(package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "An upgrade package lost its disposition.",
                        ))
                    }
                }
            }
            for prior in &prior_lock.packages {
                if candidate_lock.package(prior.package_id()).is_some() {
                    continue;
                }
                let extension = installed.get(prior.package_id()).ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A prior-only upgrade dependency is absent from the installed graph.",
                    )
                })?;
                match dispositions.get(prior.package_id()) {
                    Some(UpgradeDisposition::Remove) => {
                        prior_generations.insert(
                            prior.package_id().to_string(),
                            extension.receipt.lifecycle_generation.ok_or_else(|| {
                                package_manager_error(
                                    "use.plugin.package_generation_changed",
                                    "A removed package omitted its lifecycle generation.",
                                )
                            })?,
                        );
                        prior_manifests
                            .insert(prior.package_id().to_string(), extension.manifest.clone());
                    }
                    Some(UpgradeDisposition::Retain) => {
                        manifests
                            .insert(prior.package_id().to_string(), extension.manifest.clone());
                    }
                    _ => {
                        return Err(package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "A prior-only package has an invalid upgrade disposition.",
                        ))
                    }
                }
            }
            for manifest in manifests.values() {
                self.lifecycle.validate_manifest(manifest)?;
            }
            let root = installed.get(package_id).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The prior upgrade root disappeared before planning.",
                )
            })?;
            let (prior_surface_selections, candidate_surface_selections) =
                upgrade_surface_selections(
                    &prior_lock,
                    &candidate_lock,
                    &dispositions,
                    &installed,
                    requested_root_surfaces,
                )?;
            let capability_generation = self.current_capability_generation().await?;
            let grant_snapshot = self
                .planned_grant_snapshot(package_state_revision(capability_generation)?)
                .await?;
            let root_receipt_digest = root.receipt.descriptor_digest()?;
            let generated = upgrade_operation(
                &candidate_lock,
                &prior_lock,
                &dispositions,
                &prior_surface_selections,
                &candidate_surface_selections,
                &manifests,
                &prior_generations,
                root_receipt_digest,
                capability_generation,
                self.scope(),
                now_ms()?,
                &grant_snapshot,
                self.authorization.as_ref(),
            )?;
            let planned_at_ms = generated.envelope.plan.created_at_ms;
            let changed_manifests = manifests
                .into_iter()
                .filter(|(package_id, _)| {
                    dispositions.get(package_id) != Some(&UpgradeDisposition::Retain)
                })
                .collect();
            let pending = PendingPackageGraphOperation::planned_upgrade(
                generated.envelope,
                planned_at_ms,
                generated.generations,
                changed_manifests,
                prior_lock.clone(),
                prior_generations,
                prior_manifests,
            )?;
            pending
        };
        if pending.phase() == PackageGraphOperationPhase::Planned
            && pending_upgrade_dispositions(&pending)? != dispositions
        {
            return Err(package_manager_error(
                "use.plugin.package_generation_changed",
                "The requested root set or dependency ownership changed before upgrade admission.",
            ));
        }
        if let Some(attempt) = download_attempt.take() {
            attempt.finish().await?;
        }
        if let Some(attempt) = resolution_attempt.take() {
            attempt.finish().await?;
        }
        let pending = self
            .admit_planned_graph_operation_in_memory(pending)
            .await?;
        self.authorization.verify_plan(&pending.envelope)?;
        self.apply_control_upgrade(
            package_id,
            &prior_lock,
            &candidate_lock,
            candidate_digest,
            &pending,
            &prepared,
            &dispositions,
            maintenance.clone(),
        )
        .await
    }

}

include!("upgrade_apply.rs");
