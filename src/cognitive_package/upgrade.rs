use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    PlanPackageChangeKind, PluginOperationAction, PluginPackageLockHost, PluginReleaseChannel,
    UseResult,
};
use a3s_use_extension::{
    download_locked_remote_packages, resolve_remote_package_lock, ExtensionLifecycleIdentity,
    ExtensionLifecyclePackage, ExtensionManifest, InstalledExtension, TrustedRegistry,
};

use crate::plugin_lifecycle::{
    ExtensionGraphCapabilityLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginLifecycleOperationStatus,
    PluginPackageGraphLifecycleCoordinator, PluginPackageLifecycleUnit,
};

use super::install::verify_expected_lock;
use super::plan::{now_ms, upgrade_operation};
use super::store::PendingPackageGraphOperation;
use super::{
    current_host_target, installed_matches_lock, package_manager_error, CognitivePackageManager,
    CognitivePackageUpgradeResult, UpgradeDisposition,
};

struct PreparedUpgradePackage {
    package: ExtensionLifecyclePackage,
    manifest: ExtensionManifest,
}

impl CognitivePackageManager {
    /// Resolve and atomically upgrade one installed cognitive-package graph.
    /// Candidate generations are prepared dependency-first, published once,
    /// and exact prior generations retire only after the snapshot cutover.
    #[allow(clippy::too_many_arguments)]
    pub async fn upgrade_remote(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageUpgradeResult> {
        let candidate_lock = resolve_remote_package_lock(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            PluginPackageLockHost::new(current_host_target()?, env!("CARGO_PKG_VERSION"))?,
        )
        .await?;
        let candidate_digest = candidate_lock.descriptor_digest()?;
        verify_expected_lock(&candidate_digest, expected_package_lock_digest)?;

        let graph_store = self.graph_store();
        let pending_store = self.pending_store();
        let existing_graph = graph_store.get(package_id).await?;
        let existing_pending = pending_store
            .get(PluginOperationAction::Upgrade, package_id)
            .await?;
        let prior_lock = match (&existing_pending, &existing_graph) {
            (Some(pending), graph) => {
                validate_pending_upgrade(
                    pending,
                    &candidate_lock,
                    graph.as_ref(),
                    &self.scope_id,
                )?;
                pending.prior_package_lock.clone().ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A pending upgrade omitted its exact prior dependency lock.",
                    )
                })?
            }
            (None, Some(graph)) => graph.package_lock.clone(),
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

        let dispositions = upgrade_dispositions(&prior_lock, &candidate_lock)?;
        if existing_pending.is_none()
            && dispositions
                .values()
                .all(|disposition| *disposition == UpgradeDisposition::Retain)
        {
            let installed = self.require_published_prior(&prior_lock).await?;
            let root = installed.get(package_id).cloned().ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The retained upgrade root disappeared from its installed graph.",
                )
            })?;
            return Ok(CognitivePackageUpgradeResult {
                changed: false,
                root,
                prior_package_lock: prior_lock,
                package_lock: candidate_lock,
                package_lock_digest: candidate_digest,
                plan: None,
                added_packages: Vec::new(),
                replaced_packages: Vec::new(),
                retained_packages: dispositions.keys().cloned().collect(),
            });
        }

        let mut registries = Vec::with_capacity(dependency_registries.len() + 1);
        registries.push(root_registry.clone());
        registries.extend(dependency_registries.iter().cloned());
        let downloads = download_locked_remote_packages(&candidate_lock, &registries).await?;
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
                &self.scope_id,
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
            let installed = self.require_published_prior(&prior_lock).await?;
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
                    None => {
                        return Err(package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "An upgrade package lost its disposition.",
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
            let snapshot = self.registry.snapshot().await?;
            let generated = upgrade_operation(
                &candidate_lock,
                &prior_lock,
                &dispositions,
                &manifests,
                &prior_generations,
                root.receipt.descriptor_digest()?,
                snapshot.generation,
                &self.scope_id,
                now_ms()?,
            )?;
            let admitted_at_ms = now_ms()?;
            generated.envelope.verify_confirmed_apply(
                &generated.envelope.plan.operation_id,
                &generated.envelope.plan_digest,
                None,
                admitted_at_ms,
            )?;
            let changed_manifests = manifests
                .into_iter()
                .filter(|(package_id, _)| {
                    dispositions.get(package_id) != Some(&UpgradeDisposition::Retain)
                })
                .collect();
            let pending = PendingPackageGraphOperation::new_upgrade(
                generated.envelope,
                admitted_at_ms,
                generated.generations,
                changed_manifests,
                prior_lock.clone(),
                prior_generations,
                prior_manifests,
            )?;
            pending_store.put(&pending).await?;
            pending
        };
        for manifest in pending
            .manifests
            .values()
            .chain(pending.prior_manifests.values())
        {
            self.lifecycle.validate_manifest(manifest)?;
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
            let intent = PluginLifecycleIntent::from_manifest(
                PluginLifecycleIntentSpec {
                    operation_id: pending.envelope.plan.operation_id.clone(),
                    plan_digest: pending.envelope.plan_digest.clone(),
                    scope_id: self.scope_id.clone(),
                    package_id: package.package_id().to_string(),
                    package_digest: identity.package_digest().to_string(),
                    manifest_digest: identity.manifest_digest().to_string(),
                    generation,
                    action: match disposition {
                        UpgradeDisposition::Add => PluginLifecycleAction::Install,
                        UpgradeDisposition::Replace => PluginLifecycleAction::Upgrade,
                        UpgradeDisposition::Retain => {
                            return Err(package_manager_error(
                                "use.plugin.package_graph_invalid",
                                "A retained package cannot create a candidate lifecycle unit.",
                            ))
                        }
                    },
                },
                &prepared.manifest,
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
            if dispositions.get(package.package_id()) != Some(&UpgradeDisposition::Replace) {
                continue;
            }
            let manifest = pending
                .prior_manifests
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A replacement package omitted its prior manifest evidence.",
                    )
                })?;
            let generation = *pending
                .prior_generations
                .get(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A replacement package omitted its prior generation evidence.",
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
                        "A replacement package is absent from its admitted upgrade plan.",
                    )
                })?;
            let before = transition.before.as_ref().ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A replacement package omitted its prior selected state.",
                )
            })?;
            let identity = ExtensionLifecycleIdentity::new(
                package.package_id(),
                before.release.package_sha256.clone(),
                before.release.manifest_sha256.clone(),
                generation,
            )?;
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
        if let Err(error) = graph
            .apply_upgrade(
                &pending.envelope,
                &prior_lock,
                &candidate_units,
                &retirement_units,
                || now_ms().unwrap_or(apply_time),
            )
            .await
        {
            let mut rolled_back = true;
            for unit in &candidate_units {
                rolled_back &= unit
                    .coordinator()
                    .graph_candidate_status(unit.intent())
                    .await?
                    == Some(PluginLifecycleOperationStatus::RolledBack);
            }
            if rolled_back {
                pending_store.remove(&pending).await?;
            }
            return Err(error);
        }

        graph_store
            .replace(
                package_id,
                &prior_lock.descriptor_digest()?,
                &candidate_lock,
                now_ms()?,
            )
            .await?;
        pending_store.remove(&pending).await?;
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
        let retained_packages = package_ids(UpgradeDisposition::Retain);
        Ok(CognitivePackageUpgradeResult {
            changed: true,
            root,
            prior_package_lock: prior_lock,
            package_lock: candidate_lock,
            package_lock_digest: candidate_digest,
            plan: Some(pending.envelope),
            added_packages,
            replaced_packages,
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
}

fn upgrade_dispositions(
    prior_lock: &a3s_use_core::PluginPackageLock,
    candidate_lock: &a3s_use_core::PluginPackageLock,
) -> UseResult<BTreeMap<String, UpgradeDisposition>> {
    if prior_lock.root_package_id != candidate_lock.root_package_id
        || prior_lock.host != candidate_lock.host
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "Prior and candidate package locks belong to different roots or hosts.",
        ));
    }
    let candidate_ids = candidate_lock
        .packages
        .iter()
        .map(|package| package.package_id())
        .collect::<BTreeSet<_>>();
    if prior_lock
        .packages
        .iter()
        .any(|package| !candidate_ids.contains(package.package_id()))
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_gc_required",
            "The candidate removes a dependency node; explicit graph garbage collection is required.",
        ));
    }
    Ok(candidate_lock
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
        .collect())
}

fn validate_prepared_candidates(
    dispositions: &BTreeMap<String, UpgradeDisposition>,
    prepared: &BTreeMap<String, PreparedUpgradePackage>,
) -> UseResult<()> {
    let expected = dispositions
        .iter()
        .filter_map(|(package_id, disposition)| {
            (*disposition != UpgradeDisposition::Retain).then_some(package_id.as_str())
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

fn validate_pending_upgrade(
    pending: &PendingPackageGraphOperation,
    candidate_lock: &a3s_use_core::PluginPackageLock,
    graph: Option<&super::store::InstalledPackageGraph>,
    scope_id: &str,
) -> UseResult<()> {
    pending.validate()?;
    let prior = pending.prior_package_lock.as_ref().ok_or_else(|| {
        package_manager_error(
            "use.plugin.package_graph_invalid",
            "A pending upgrade omitted its prior dependency lock.",
        )
    })?;
    if pending.envelope.plan.action != PluginOperationAction::Upgrade
        || pending.envelope.package_lock.as_ref() != Some(candidate_lock)
        || pending.envelope.plan.scope.id != scope_id
        || graph.is_none_or(|graph| {
            graph.package_lock != *prior && graph.package_lock != *candidate_lock
        })
        || pending
            .envelope
            .plan
            .packages
            .iter()
            .any(|transition| transition.change == PlanPackageChangeKind::Remove)
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_busy",
            "The pending cognitive-package upgrade no longer matches the resolved or installed graph.",
        ));
    }
    Ok(())
}
