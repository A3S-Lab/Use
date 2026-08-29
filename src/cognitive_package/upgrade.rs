use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    PluginOperationAction, PluginPackageLock, PluginReleaseChannel, PluginSurfaceRef, UseResult,
};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionManifest,
    ExtensionRegistrySnapshot, InstalledExtension, TrustedRegistry,
};

use crate::plugin_lifecycle::{
    ExtensionGraphCapabilityLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginLifecycleOperationStatus,
    PluginPackageGraphLifecycleCoordinator, PluginPackageLifecycleUnit,
};

use super::download_attempt::PendingPackageDownloadAttempt;
use super::install::verify_expected_lock;
use super::plan::{now_ms, package_state_revision, state_surface_refs, upgrade_operation};
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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

        let snapshot_store = self.snapshot_store();
        let pending_store = self.pending_store();
        let existing_graph = snapshot_store.get(package_id).await?;
        let existing_pending = pending_store
            .get(PluginOperationAction::Upgrade, package_id)
            .await?;
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

        let installed_locks = snapshot_store.list().await?;
        let installed_extensions = self.registry.list().await?;
        let snapshot = self.registry.snapshot().await?;
        let mut dispositions = upgrade_dispositions(
            &prior_lock,
            &candidate_lock,
            &installed_locks,
            &installed_extensions,
            &snapshot,
        )?;
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
            let current = self.registry.get(package_id).await?.ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_missing",
                    "The installed upgrade root disappeared before surface selection.",
                )
            })?;
            if dispositions.get(package_id) == Some(&UpgradeDisposition::Retain)
                && current.selected_surfaces()? != expected
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
            let snapshot = self.registry.snapshot().await?;
            let grant_snapshot = self
                .grant_store()
                .snapshot_scope(
                    &self.scope().id,
                    package_state_revision(snapshot.generation)?,
                )
                .await?;
            let generated = upgrade_operation(
                &candidate_lock,
                &prior_lock,
                &dispositions,
                &prior_surface_selections,
                &candidate_surface_selections,
                &manifests,
                &prior_generations,
                root.receipt.descriptor_digest()?,
                snapshot.generation,
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
            pending_store.put(&pending).await?;
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
            .admit_planned_graph_operation(&pending_store, pending)
            .await?;
        self.authorization.verify_plan(&pending.envelope)?;
        for manifest in pending.manifests.values() {
            self.lifecycle.validate_manifest(manifest)?;
        }
        for manifest in pending.prior_manifests.values() {
            self.lifecycle.validate_manifest_for_retirement(manifest)?;
        }

        let mut candidate_units = Vec::with_capacity(pending.generations.len());
        for package in candidate_lock.install_order()? {
            let disposition = dispositions.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A candidate package lost its upgrade disposition.",
                )
            })?;
            if *disposition == UpgradeDisposition::Retain {
                continue;
            }
            let prepared = prepared.remove(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A changed upgrade package has no prepared candidate.",
                )
            })?;
            let generation = *pending
                .generations
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A changed upgrade package has no admitted candidate generation.",
                    )
                })?;
            let identity = ExtensionLifecycleIdentity::new(
                package.package_id(),
                prepared.package.package_digest(),
                prepared.package.manifest_digest(),
                generation,
            )?;
            let retained_ui_state_surfaces = match disposition {
                UpgradeDisposition::Replace => {
                    let prior_manifest = pending
                        .prior_manifests
                        .get(package.package_id())
                        .ok_or_else(|| {
                            package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A replacement package omitted its prior UI inventory.",
                            )
                        })?;
                    common_ui_surface_ids(prior_manifest, &prepared.manifest)
                }
                UpgradeDisposition::Add => Vec::new(),
                UpgradeDisposition::Remove | UpgradeDisposition::Retain => {
                    return Err(package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A non-candidate package attempted to retain candidate UI state.",
                    ))
                }
            };
            let after = pending
                .envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
                .and_then(|transition| transition.after.as_ref())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A changed upgrade package omitted its selected candidate state.",
                    )
                })?;
            let selected_surfaces = state_surface_refs(after);
            let intent = PluginLifecycleIntent::from_manifest_selection(
                PluginLifecycleIntentSpec {
                    operation_id: pending.envelope.plan.operation_id.clone(),
                    plan_digest: pending.envelope.plan_digest.clone(),
                    scope: self.scope().clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: identity.package_digest().to_string(),
                    manifest_digest: identity.manifest_digest().to_string(),
                    generation,
                    action: match disposition {
                        UpgradeDisposition::Add => PluginLifecycleAction::Install,
                        UpgradeDisposition::Replace => PluginLifecycleAction::Upgrade,
                        UpgradeDisposition::Remove => {
                            return Err(package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A removed package cannot create a candidate lifecycle unit.",
                            ))
                        }
                        UpgradeDisposition::Retain => {
                            return Err(package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A retained package cannot create a candidate lifecycle unit.",
                            ))
                        }
                    },
                    retained_ui_state_surfaces,
                },
                &prepared.manifest,
                &selected_surfaces,
            )?;
            candidate_units.push(PluginPackageLifecycleUnit::new(
                self.lifecycle.install_coordinator(
                    self.registry.clone(),
                    prepared.package,
                    self.registry.lifecycle_package_root(&identity),
                )?,
                intent,
                prepared.manifest,
            )?);
        }

        let mut retirement_units = Vec::with_capacity(pending.prior_generations.len());
        for package in prior_lock.removal_order()? {
            if !matches!(
                dispositions.get(package.package_id()),
                Some(UpgradeDisposition::Replace | UpgradeDisposition::Remove)
            ) {
                continue;
            }
            let manifest = pending
                .prior_manifests
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A retired package omitted its prior manifest evidence.",
                    )
                })?;
            let generation = *pending
                .prior_generations
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A retired package omitted its prior generation evidence.",
                    )
                })?;
            let transition = pending
                .envelope
                .plan
                .packages
                .iter()
                .find(|transition| transition.package_id == package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A retired package is absent from its admitted upgrade plan.",
                    )
                })?;
            let before = transition.before.as_ref().ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A retired package omitted its prior selected state.",
                )
            })?;
            let identity = ExtensionLifecycleIdentity::new(
                package.package_id(),
                before.release.package_sha256.clone(),
                before.release.manifest_sha256.clone(),
                generation,
            )?;
            let retained_ui_state_surfaces =
                if dispositions.get(package.package_id()) == Some(&UpgradeDisposition::Replace) {
                    let candidate_manifest =
                        pending.manifests.get(package.package_id()).ok_or_else(|| {
                            package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A replacement package omitted its candidate UI inventory.",
                            )
                        })?;
                    common_ui_surface_ids(manifest, candidate_manifest)
                } else {
                    Vec::new()
                };
            let selected_surfaces = state_surface_refs(before);
            let intent = PluginLifecycleIntent::from_manifest_selection(
                PluginLifecycleIntentSpec {
                    operation_id: pending.envelope.plan.operation_id.clone(),
                    plan_digest: pending.envelope.plan_digest.clone(),
                    scope: self.scope().clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: identity.package_digest().to_string(),
                    manifest_digest: identity.manifest_digest().to_string(),
                    generation,
                    action: PluginLifecycleAction::Uninstall,
                    retained_ui_state_surfaces,
                },
                manifest,
                &selected_surfaces,
            )?;
            if dispositions.get(package.package_id()) == Some(&UpgradeDisposition::Remove) {
                match self.registry.get_lifecycle_generation(&identity).await? {
                    Some(extension) => {
                        validate_removed_generation(&extension, manifest, &identity)?;
                    }
                    None => self.validate_missing_retirement_replay(&intent).await?,
                }
            }
            retirement_units.push(PluginPackageLifecycleUnit::new(
                self.lifecycle.uninstall_coordinator(
                    self.registry.clone(),
                    self.registry.lifecycle_package_root(&identity),
                )?,
                intent,
                manifest.clone(),
            )?);
        }

        let apply_time = now_ms()?;
        let graph = PluginPackageGraphLifecycleCoordinator::new(std::sync::Arc::new(
            ExtensionGraphCapabilityLifecycleHost::new(self.registry.clone()),
        ));
        let grant_unit = pending
            .authorization
            .lifecycle_unit(self.grant_store(), &pending.envelope)?;
        let apply_result = match grant_unit.as_ref() {
            Some(grants) => {
                graph
                    .apply_upgrade_with_grants(
                        &pending.envelope,
                        &prior_lock,
                        &candidate_units,
                        &retirement_units,
                        grants,
                        || now_ms().unwrap_or(apply_time),
                    )
                    .await
            }
            None => {
                graph
                    .apply_upgrade(
                        &pending.envelope,
                        &prior_lock,
                        &candidate_units,
                        &retirement_units,
                        || now_ms().unwrap_or(apply_time),
                    )
                    .await
            }
        };
        if let Err(error) = apply_result {
            let mut rolled_back = true;
            for unit in &candidate_units {
                rolled_back &= unit
                    .coordinator()
                    .graph_candidate_status(unit.intent())
                    .await?
                    == Some(PluginLifecycleOperationStatus::RolledBack);
            }
            if rolled_back {
                self.retain_and_remove_graph_operation(
                    &pending_store,
                    &pending,
                    super::PluginRetainedOperationOutcome::RolledBack,
                )
                .await?;
            }
            return Err(error);
        }

        snapshot_store
            .replace(
                package_id,
                &prior_lock.descriptor_digest()?,
                &candidate_lock,
                now_ms()?,
            )
            .await?;
        self.retain_and_remove_graph_operation(
            &pending_store,
            &pending,
            super::PluginRetainedOperationOutcome::Completed,
        )
        .await?;
        let root = self.registry.get(package_id).await?.ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "The upgraded cognitive-package root is missing after graph cutover.",
            )
        })?;
        let ordered = candidate_lock.install_order()?;
        let package_ids = |kind| {
            ordered
                .iter()
                .filter(|package| dispositions.get(package.package_id()) == Some(&kind))
                .map(|package| package.package_id().to_string())
                .collect::<Vec<_>>()
        };
        let added_packages = package_ids(UpgradeDisposition::Add);
        let replaced_packages = package_ids(UpgradeDisposition::Replace);
        let removed_packages = prior_lock
            .removal_order()?
            .into_iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&UpgradeDisposition::Remove)
            })
            .map(|package| package.package_id().to_string())
            .collect();
        let retained_packages = dispositions
            .iter()
            .filter_map(|(package_id, disposition)| {
                (*disposition == UpgradeDisposition::Retain).then_some(package_id.clone())
            })
            .collect();
        Ok(CognitivePackageUpgradeResult {
            changed: true,
            root,
            prior_package_lock: prior_lock,
            package_lock: candidate_lock,
            package_lock_digest: candidate_digest,
            plan: Some(pending.envelope),
            added_packages,
            replaced_packages,
            removed_packages,
            retained_packages,
        })
    }

    async fn require_published_prior(
        &self,
        prior_lock: &a3s_use_core::PluginPackageLock,
    ) -> UseResult<BTreeMap<String, InstalledExtension>> {
        let snapshot = self.registry.snapshot().await?;
        let mut installed = BTreeMap::new();
        for package in &prior_lock.packages {
            let extension = self
                .registry
                .get(package.package_id())
                .await?
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Prior dependency '{}' is missing before upgrade.",
                            package.package_id()
                        ),
                    )
                })?;
            if !installed_matches_lock(&extension, &package.catalog)?
                || !extension.receipt.enabled
                || !snapshot.routes.iter().any(|route| {
                    route.package_id == extension.receipt.package_id
                        && route.enabled
                        && route.lifecycle_generation == extension.receipt.lifecycle_generation
                        && route.package_sha256 == extension.receipt.package_sha256
                        && route.manifest_sha256 == extension.receipt.manifest_sha256
                })
            {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Prior dependency '{}' is not the exact published lock generation.",
                        package.package_id()
                    ),
                ));
            }
            installed.insert(package.package_id().to_string(), extension);
        }
        Ok(installed)
    }

    async fn add_published_candidate_retentions(
        &self,
        prior_lock: &a3s_use_core::PluginPackageLock,
        candidate_lock: &a3s_use_core::PluginPackageLock,
        dispositions: &BTreeMap<String, UpgradeDisposition>,
        installed: &mut BTreeMap<String, InstalledExtension>,
    ) -> UseResult<()> {
        let snapshot = self.registry.snapshot().await?;
        for candidate in &candidate_lock.packages {
            if prior_lock.package(candidate.package_id()).is_some()
                || dispositions.get(candidate.package_id()) != Some(&UpgradeDisposition::Retain)
            {
                continue;
            }
            let extension = self
                .registry
                .get(candidate.package_id())
                .await?
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Shared candidate dependency '{}' disappeared before upgrade planning.",
                            candidate.package_id()
                        ),
                    )
                })?;
            if !installed_matches_lock(&extension, &candidate.catalog)?
                || !extension_is_exact_published(&snapshot, &extension)
            {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Shared candidate dependency '{}' is not its exact published lock generation.",
                        candidate.package_id()
                    ),
                ));
            }
            if installed
                .insert(candidate.package_id().to_string(), extension)
                .is_some()
            {
                return Err(package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A shared candidate dependency overlaps the prior installed graph.",
                ));
            }
        }
        Ok(())
    }

    async fn validate_missing_retirement_replay(
        &self,
        intent: &PluginLifecycleIntent,
    ) -> UseResult<()> {
        let journal = crate::plugin_lifecycle::PluginLifecycleJournalStore::from_extension_paths(
            self.registry.paths(),
        );
        let exact_journal = journal
            .load_active(&intent.scope, &intent.package_id)
            .await?
            .is_some_and(|record| record.intent == *intent);
        let route_absent = self
            .registry
            .snapshot()
            .await?
            .routes
            .iter()
            .all(|binding| {
                binding.package_id != intent.package_id
                    || binding.lifecycle_generation != Some(intent.generation)
                    || binding.package_sha256.as_deref()
                        != intent.package_digest.strip_prefix("sha256:")
                    || binding.manifest_sha256
                        != intent
                            .manifest_digest
                            .strip_prefix("sha256:")
                            .unwrap_or_default()
            });
        if exact_journal && route_absent {
            Ok(())
        } else {
            Err(package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                format!(
                    "Missing removed package '{}' has no exact route-free upgrade journal to replay.",
                    intent.package_id
                ),
            ))
        }
    }
}

#[allow(clippy::type_complexity)]
fn upgrade_surface_selections(
    prior_lock: &a3s_use_core::PluginPackageLock,
    candidate_lock: &a3s_use_core::PluginPackageLock,
    dispositions: &BTreeMap<String, UpgradeDisposition>,
    installed: &BTreeMap<String, InstalledExtension>,
    requested_root_surfaces: Option<&[PluginSurfaceRef]>,
) -> UseResult<(
    BTreeMap<String, Vec<PluginSurfaceRef>>,
    BTreeMap<String, Vec<PluginSurfaceRef>>,
)> {
    let mut prior = BTreeMap::new();
    for package in &prior_lock.packages {
        let extension = installed.get(package.package_id()).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "A prior package is missing its installed surface evidence.",
            )
        })?;
        prior.insert(
            package.package_id().to_string(),
            extension.selected_surfaces()?,
        );
    }

    let mut candidate = BTreeMap::new();
    for package in &candidate_lock.packages {
        let selected = match dispositions.get(package.package_id()) {
            Some(UpgradeDisposition::Retain) => installed
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A retained candidate is missing its installed surface evidence.",
                    )
                })?
                .selected_surfaces()?,
            Some(UpgradeDisposition::Add | UpgradeDisposition::Replace) => {
                let requested = requested_root_surfaces
                    .filter(|_| package.package_id() == candidate_lock.root_package_id)
                    .unwrap_or(&[]);
                if requested_root_surfaces.is_none() {
                    super::all_catalog_surfaces(package)
                } else {
                    package
                        .catalog
                        .record
                        .resolve_surfaces(requested)?
                        .into_iter()
                        .map(|surface| surface.reference())
                        .collect()
                }
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
                    "A candidate package has no upgrade disposition.",
                ))
            }
        };
        candidate.insert(package.package_id().to_string(), selected);
    }
    Ok((prior, candidate))
}

fn common_ui_surface_ids(prior: &ExtensionManifest, candidate: &ExtensionManifest) -> Vec<String> {
    let prior = prior
        .ui
        .iter()
        .map(|surface| surface.id.as_str())
        .collect::<BTreeSet<_>>();
    candidate
        .ui
        .iter()
        .filter(|surface| prior.contains(surface.id.as_str()))
        .map(|surface| surface.id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn validate_removed_generation(
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
                "Removed package '{}' changed generation before graph garbage collection.",
                extension.receipt.package_id
            ),
        ));
    }
    Ok(())
}

fn extension_is_exact_published(
    snapshot: &ExtensionRegistrySnapshot,
    extension: &InstalledExtension,
) -> bool {
    extension.receipt.enabled
        && snapshot.routes.iter().any(|route| {
            route.package_id == extension.receipt.package_id
                && route.enabled
                && route.lifecycle_generation == extension.receipt.lifecycle_generation
                && route.package_sha256 == extension.receipt.package_sha256
                && route.manifest_sha256 == extension.receipt.manifest_sha256
        })
}

fn upgrade_dispositions(
    prior_lock: &a3s_use_core::PluginPackageLock,
    candidate_lock: &a3s_use_core::PluginPackageLock,
    installed_locks: &[PluginPackageLock],
    installed_extensions: &[InstalledExtension],
    snapshot: &ExtensionRegistrySnapshot,
) -> UseResult<BTreeMap<String, UpgradeDisposition>> {
    if prior_lock.root_package_id != candidate_lock.root_package_id
        || prior_lock.host != candidate_lock.host
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "Prior and candidate package locks belong to different roots or hosts.",
        ));
    }
    let mut dispositions = candidate_lock
        .packages
        .iter()
        .map(|candidate| {
            let disposition = match prior_lock.package(candidate.package_id()) {
                None => UpgradeDisposition::Add,
                Some(prior) if prior.catalog == candidate.catalog => UpgradeDisposition::Retain,
                Some(_) => UpgradeDisposition::Replace,
            };
            (candidate.package_id().to_string(), disposition)
        })
        .collect::<BTreeMap<_, _>>();
    for prior in &prior_lock.packages {
        dispositions
            .entry(prior.package_id().to_string())
            .or_insert(UpgradeDisposition::Remove);
    }

    let installed_by_id = installed_extensions
        .iter()
        .map(|extension| (extension.receipt.package_id.as_str(), extension))
        .collect::<BTreeMap<_, _>>();
    for candidate in &candidate_lock.packages {
        if prior_lock.package(candidate.package_id()).is_some() {
            continue;
        }
        let Some(extension) = installed_by_id.get(candidate.package_id()).copied() else {
            continue;
        };
        if !installed_matches_lock(extension, &candidate.catalog)? {
            return Err(package_manager_error(
                "use.plugin.package_graph_shared_upgrade_required",
                format!(
                    "Candidate dependency '{}' is already installed at a different exact release and cannot be replaced by this graph upgrade.",
                    candidate.package_id()
                ),
            ));
        }
        if !extension_is_exact_published(snapshot, extension) {
            return Err(package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                format!(
                    "Candidate dependency '{}' exists but is not its exact published generation.",
                    candidate.package_id()
                ),
            ));
        }
        dispositions.insert(
            candidate.package_id().to_string(),
            UpgradeDisposition::Retain,
        );
    }

    let prior_ids = prior_lock
        .packages
        .iter()
        .map(|package| package.package_id())
        .collect::<BTreeSet<_>>();
    let mut externally_retained = BTreeSet::new();
    for installed_lock in installed_locks
        .iter()
        .filter(|lock| lock.root_package_id != prior_lock.root_package_id)
    {
        for package in &installed_lock.packages {
            if !prior_ids.contains(package.package_id()) {
                continue;
            }
            if dispositions.get(package.package_id()) == Some(&UpgradeDisposition::Replace) {
                return Err(package_manager_error(
                    "use.plugin.package_graph_shared_upgrade_required",
                    format!(
                        "Dependency '{}' is locked by installed root '{}' and cannot be replaced by an uncoordinated graph upgrade.",
                        package.package_id(), installed_lock.root_package_id
                    ),
                ));
            }
            externally_retained.insert(package.package_id().to_string());
        }
    }

    let operation_ids = prior_lock
        .packages
        .iter()
        .chain(&candidate_lock.packages)
        .map(|package| package.package_id())
        .collect::<BTreeSet<_>>();
    for extension in installed_extensions {
        if operation_ids.contains(extension.receipt.package_id.as_str()) {
            continue;
        }
        for dependency in &extension.manifest.dependencies {
            if !prior_ids.contains(dependency.package_id.as_str()) {
                continue;
            }
            if dispositions.get(&dependency.package_id) == Some(&UpgradeDisposition::Replace) {
                return Err(package_manager_error(
                    "use.plugin.package_graph_shared_upgrade_required",
                    format!(
                        "Dependency '{}' is referenced by installed package '{}' and cannot be replaced by an uncoordinated graph upgrade.",
                        dependency.package_id, extension.receipt.package_id
                    ),
                ));
            }
            externally_retained.insert(dependency.package_id.clone());
        }
    }

    loop {
        let before = externally_retained.len();
        for package_id in externally_retained.clone() {
            if let Some(package) = prior_lock.package(&package_id) {
                externally_retained.extend(
                    package
                        .dependencies
                        .iter()
                        .map(|dependency| dependency.package_id.clone()),
                );
            }
        }
        if externally_retained.len() == before {
            break;
        }
    }
    for package_id in externally_retained {
        let Some(disposition) = dispositions.get_mut(&package_id) else {
            continue;
        };
        match *disposition {
            UpgradeDisposition::Remove => *disposition = UpgradeDisposition::Retain,
            UpgradeDisposition::Replace => {
                return Err(package_manager_error(
                    "use.plugin.package_graph_shared_upgrade_required",
                    format!(
                        "Dependency '{package_id}' remains transitively owned by another installed package and cannot be replaced independently."
                    ),
                ));
            }
            UpgradeDisposition::Add | UpgradeDisposition::Retain => {}
        }
    }
    Ok(dispositions)
}

fn validate_prepared_candidates(
    dispositions: &BTreeMap<String, UpgradeDisposition>,
    prepared: &BTreeMap<String, PreparedUpgradePackage>,
) -> UseResult<()> {
    let expected = dispositions
        .iter()
        .filter_map(|(package_id, disposition)| {
            matches!(
                disposition,
                UpgradeDisposition::Add | UpgradeDisposition::Replace
            )
            .then_some(package_id.as_str())
        })
        .collect::<BTreeSet<_>>();
    let actual = prepared.keys().map(String::as_str).collect::<BTreeSet<_>>();
    if expected != actual {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "The prepared upgrade set does not equal the changed dependency closure.",
        ));
    }
    Ok(())
}
