// Control-path upgrade apply helpers (included into upgrade).

impl CognitivePackageManager {
    async fn apply_control_upgrade(
        &self,
        package_id: &str,
        prior_lock: &PluginPackageLock,
        candidate_lock: &PluginPackageLock,
        candidate_digest: String,
        pending: &PendingPackageGraphOperation,
        prepared: &BTreeMap<String, PreparedUpgradePackage>,
        dispositions: &BTreeMap<String, UpgradeDisposition>,
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<CognitivePackageUpgradeResult> {
        for manifest in pending.manifests.values() {
            self.lifecycle.validate_manifest(manifest)?;
        }
        for manifest in pending.prior_manifests.values() {
            self.lifecycle.validate_manifest_for_retirement(manifest)?;
        }
        let artifact_store = self.registry.paths().artifact_store();
        let artifact_admission = artifact_store.acquire_reference_admission().await?;
        for prepared in prepared.values() {
            if pending.manifests.get(&prepared.manifest.package_id) != Some(&prepared.manifest) {
                return Err(package_manager_error(
                    "use.plugin.package_changed",
                    format!(
                        "Prepared package '{}' no longer matches its pending admitted manifest.",
                        prepared.manifest.package_id
                    ),
                ));
            }
            artifact_store
                .admit_prepared_package(&artifact_admission, &prepared.package)
                .await?;
        }
        // Release reachability admission before Control drain (nested shared
        // reachability locks deadlock on Windows under effect owners).
        drop(artifact_admission);
        let publications = self.lifecycle.runtime_plan_publications()?;
        super::control_authority::require_control_runtime_readiness_for_publications(
            self.lifecycle.control_runtime_readiness().as_ref(),
            &publications,
        )?;
        let control = self.ensure_control().await?;
        let snapshot = super::control_authority::apply_pending_through_control(
            control,
            pending,
            &publications,
            maintenance,
        )
        .await?;

        let root_selection = snapshot.package_selection(package_id).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "The upgraded cognitive-package root is missing after Control commit.",
            )
        })?;
        let root = if let Some(prepared) = prepared.get(package_id) {
            let package_root =
                artifact_store.expanded_package_path(prepared.package.package_digest())?;
            let generation = *pending
                .generations
                .get(package_id)
                .unwrap_or(&root_selection.state_generation);
            InstalledExtension {
                receipt: a3s_use_extension::ExtensionReceipt {
                    schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
                    installation: self.scope().clone(),
                    package_id: package_id.to_string(),
                    component_id: format!("use/{package_id}"),
                    route_alias: prepared.manifest.route_alias.clone(),
                    version: prepared.manifest.version.clone(),
                    package_root,
                    manifest_sha256: prepared.package.manifest_digest().to_string(),
                    package_sha256: Some(prepared.package.package_digest().to_string()),
                    trust: prepared.package.trust(),
                    registry: prepared.package.registry().cloned(),
                    verified_catalog: prepared.package.verified_catalog().cloned(),
                    planning_bundle: prepared.package.planning_bundle().cloned(),
                    selected_surfaces: root_selection.selected_surfaces.clone(),
                    installed_at_unix: 1,
                    enabled: root_selection.enabled,
                    lifecycle_generation: Some(generation),
                },
                manifest: prepared.manifest.clone(),
            }
        } else {
            let verified = artifact_store
                .acquire_verified_package(&root_selection.package.catalog)
                .await?;
            let package_digest = verified.package_digest();
            let package_root = artifact_store.expanded_package_path(package_digest)?;
            InstalledExtension {
                receipt: a3s_use_extension::ExtensionReceipt {
                    schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
                    installation: self.scope().clone(),
                    package_id: package_id.to_string(),
                    component_id: format!("use/{package_id}"),
                    route_alias: verified.manifest().route_alias.clone(),
                    version: root_selection.package.version().to_string(),
                    package_root,
                    manifest_sha256: verified.manifest_digest().to_string(),
                    package_sha256: Some(package_digest.to_string()),
                    trust: a3s_use_extension::ExtensionTrust::RegistryTuf,
                    registry: None,
                    verified_catalog: Some(root_selection.package.catalog.clone()),
                    planning_bundle: None,
                    selected_surfaces: root_selection.selected_surfaces.clone(),
                    installed_at_unix: 1,
                    enabled: root_selection.enabled,
                    lifecycle_generation: Some(root_selection.state_generation),
                },
                manifest: verified.manifest().clone(),
            }
        };

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
            .filter_map(|(id, disposition)| {
                (*disposition == UpgradeDisposition::Retain).then_some(id.clone())
            })
            .collect();
        Ok(CognitivePackageUpgradeResult {
            changed: true,
            root,
            prior_package_lock: prior_lock.clone(),
            package_lock: candidate_lock.clone(),
            package_lock_digest: candidate_digest,
            plan: Some(pending.envelope.clone()),
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
        self.require_published_prior_control(prior_lock).await
    }

    async fn require_published_prior_control(
        &self,
        prior_lock: &a3s_use_core::PluginPackageLock,
    ) -> UseResult<BTreeMap<String, InstalledExtension>> {
        let control = self.ensure_control().await?;
        let snapshot = control.current_snapshot().await?.ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                "Control Store has no installation snapshot before upgrade.",
            )
        })?;
        let artifact_store = self.registry.paths().artifact_store();
        let mut installed = BTreeMap::new();
        for package in &prior_lock.packages {
            let selection = snapshot
                .package_selection(package.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Prior dependency '{}' is missing from the Control snapshot.",
                            package.package_id()
                        ),
                    )
                })?;
            if &selection.package != package || !selection.enabled {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Prior dependency '{}' is not the exact enabled Control lock generation.",
                        package.package_id()
                    ),
                ));
            }
            let verified = artifact_store
                .acquire_verified_package(&selection.package.catalog)
                .await?;
            let package_digest = verified.package_digest();
            let package_root = artifact_store.expanded_package_path(package_digest)?;
            let manifest_sha256 = selection
                .package
                .catalog
                .record
                .package
                .manifest_sha256
                .clone()
                .unwrap_or_else(|| verified.manifest_digest().to_string());
            let extension = InstalledExtension {
                receipt: a3s_use_extension::ExtensionReceipt {
                    schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
                    installation: self.scope().clone(),
                    package_id: package.package_id().to_string(),
                    component_id: format!("use/{}", package.package_id()),
                    route_alias: verified.manifest().route_alias.clone(),
                    version: package.version().to_string(),
                    package_root,
                    manifest_sha256,
                    package_sha256: Some(package_digest.to_string()),
                    trust: a3s_use_extension::ExtensionTrust::RegistryTuf,
                    registry: None,
                    verified_catalog: Some(selection.package.catalog.clone()),
                    planning_bundle: None,
                    selected_surfaces: selection.selected_surfaces.clone(),
                    installed_at_unix: 1,
                    enabled: selection.enabled,
                    lifecycle_generation: Some(selection.state_generation),
                },
                manifest: verified.manifest().clone(),
            };
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
        self.add_published_candidate_retentions_control(
            prior_lock,
            candidate_lock,
            dispositions,
            installed,
        )
        .await
    }

    async fn add_published_candidate_retentions_control(
        &self,
        prior_lock: &a3s_use_core::PluginPackageLock,
        candidate_lock: &a3s_use_core::PluginPackageLock,
        dispositions: &BTreeMap<String, UpgradeDisposition>,
        installed: &mut BTreeMap<String, InstalledExtension>,
    ) -> UseResult<()> {
        let control = self.ensure_control().await?;
        let snapshot = control.current_snapshot().await?.ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                "Control Store has no installation snapshot before upgrade planning.",
            )
        })?;
        let artifact_store = self.registry.paths().artifact_store();
        for candidate in &candidate_lock.packages {
            if prior_lock.package(candidate.package_id()).is_some()
                || dispositions.get(candidate.package_id()) != Some(&UpgradeDisposition::Retain)
            {
                continue;
            }
            let selection = snapshot
                .package_selection(candidate.package_id())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_reconcile_required",
                        format!(
                            "Shared candidate dependency '{}' disappeared before upgrade planning.",
                            candidate.package_id()
                        ),
                    )
                })?;
            if &selection.package != candidate || !selection.enabled {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Shared candidate dependency '{}' is not its exact enabled Control generation.",
                        candidate.package_id()
                    ),
                ));
            }
            let verified = artifact_store
                .acquire_verified_package(&selection.package.catalog)
                .await?;
            let package_digest = verified.package_digest();
            let package_root = artifact_store.expanded_package_path(package_digest)?;
            let extension = InstalledExtension {
                receipt: a3s_use_extension::ExtensionReceipt {
                    schema_version: a3s_use_extension::EXTENSION_RECEIPT_SCHEMA_VERSION,
                    installation: self.scope().clone(),
                    package_id: candidate.package_id().to_string(),
                    component_id: format!("use/{}", candidate.package_id()),
                    route_alias: verified.manifest().route_alias.clone(),
                    version: candidate.version().to_string(),
                    package_root,
                    manifest_sha256: selection
                        .package
                        .catalog
                        .record
                        .package
                        .manifest_sha256
                        .clone()
                        .unwrap_or_else(|| verified.manifest_digest().to_string()),
                    package_sha256: Some(package_digest.to_string()),
                    trust: a3s_use_extension::ExtensionTrust::RegistryTuf,
                    registry: None,
                    verified_catalog: Some(selection.package.catalog.clone()),
                    planning_bundle: None,
                    selected_surfaces: selection.selected_surfaces.clone(),
                    installed_at_unix: 1,
                    enabled: selection.enabled,
                    lifecycle_generation: Some(selection.state_generation),
                },
                manifest: verified.manifest().clone(),
            };
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

fn extension_is_exact_published(
    snapshot: &ExtensionRegistrySnapshot,
    extension: &InstalledExtension,
) -> bool {
    extension.receipt.enabled
        && snapshot.packages.iter().any(|binding| {
            binding.package_id == extension.receipt.package_id
                && binding.enabled
                && binding.lifecycle_generation == extension.receipt.lifecycle_generation
                && binding.package_sha256 == extension.receipt.package_sha256
                && binding.manifest_sha256 == extension.receipt.manifest_sha256
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
