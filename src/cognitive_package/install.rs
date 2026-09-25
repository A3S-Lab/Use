use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use a3s_use_core::{
    PlanScope, PluginOperationAction, PluginReleaseChannel, PluginSurfaceRef, UseResult,
};
use a3s_use_extension::{
    ExtensionLifecyclePackage, ExtensionManifest, ExtensionReceipt, ExtensionTrust,
    InstalledExtension, TrustedRegistry, EXTENSION_RECEIPT_SCHEMA_VERSION,
};

use super::download_attempt::PendingPackageDownloadAttempt;
use super::plan::{install_operation, now_ms, package_state_revision};
use super::registry_access::{download_selected_packages, resolve_package_lock, RegistryAccess};
use super::resolution_attempt::PendingPackageResolutionAttempt;
use super::store::PendingPackageGraphOperation;
use super::{
    package_manager_error, CognitivePackageInstallResult, CognitivePackageManager,
    InstallDisposition,
};

struct PreparedInstallPackage {
    package: ExtensionLifecyclePackage,
    manifest: ExtensionManifest,
}

impl CognitivePackageManager {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn install_remote_with_access(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
        access: RegistryAccess,
        requested_root_surfaces: Option<&[PluginSurfaceRef]>,
    ) -> UseResult<CognitivePackageInstallResult> {
        // Hold the installation mutation lock for the whole install. Acquire the
        // shared maintenance fence only after Registry resolve/download so TUF
        // HTTP work does not sit under the fence (and so Control drain can nest
        // shared maintenance safely once package bytes exist).
        let _mutation = self.installation_mutation_lock().acquire().await?;
        self.require_graph_mutation_domain(PluginOperationAction::Install, package_id)
            .await?;
        let mut resolution_attempt = Some(
            self.resolution_attempt_store()
                .begin(PendingPackageResolutionAttempt::new(
                    self.scope().clone(),
                    PluginOperationAction::Install,
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
        let lock = match resolve_package_lock(
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
        let lock_digest = lock.descriptor_digest()?;
        verify_expected_lock(&lock_digest, expected_package_lock_digest)?;
        let (dispositions, installed) = self.install_dispositions(&lock).await?;
        let surface_selections =
            install_surface_selections(&lock, &dispositions, &installed, requested_root_surfaces)?;

        if dispositions.get(&lock.root_package_id) == Some(&InstallDisposition::Retain) {
            if dispositions
                .values()
                .any(|value| *value != InstallDisposition::Retain)
            {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Published root '{}' no longer has its complete installed dependency closure.",
                        lock.root_package_id
                    ),
                ));
            }
            self.verify_published_closure(&lock, &installed).await?;
            let root = installed
                .get(&lock.root_package_id)
                .cloned()
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "The retained root package disappeared during graph adoption.",
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
            return Ok(CognitivePackageInstallResult {
                changed: false,
                root,
                package_lock: lock,
                package_lock_digest: lock_digest,
                plan: None,
                installed_packages: Vec::new(),
                retained_packages: dispositions.keys().cloned().collect(),
            });
        }

        let mut registries = Vec::with_capacity(dependency_registries.len() + 1);
        registries.push(root_registry.clone());
        registries.extend(dependency_registries.iter().cloned());
        let selected_downloads: BTreeSet<String> = dispositions
            .iter()
            .filter_map(|(package_id, disposition)| {
                (*disposition == InstallDisposition::Add).then_some(package_id.clone())
            })
            .collect();
        let download_store = self.download_attempt_store();
        let mut download_attempt = Some(
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
                        PluginOperationAction::Install,
                        lock.clone(),
                        selected_downloads.clone(),
                        now_ms()?,
                    )?,
                )
                .await?,
        );
        let downloads =
            download_selected_packages(access, &lock, &registries, &selected_downloads).await?;
        let mut prepared = Vec::new();
        let mut manifests = installed
            .iter()
            .filter(|(package_id, _)| {
                dispositions.get(*package_id) == Some(&InstallDisposition::Retain)
            })
            .map(|(package_id, extension)| (package_id.clone(), extension.manifest.clone()))
            .collect::<BTreeMap<_, _>>();
        for download in downloads {
            let package_id = download.resolved().package_id.clone();
            if dispositions.get(&package_id) == Some(&InstallDisposition::Retain) {
                continue;
            }
            let package = ExtensionLifecyclePackage::prepare_remote(&package_id, download).await?;
            let manifest = package.manifest().clone();
            if manifests
                .insert(package_id.clone(), manifest.clone())
                .is_some()
            {
                return Err(package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A prepared package appears more than once in the dependency closure.",
                ));
            }
            prepared.push(PreparedInstallPackage { package, manifest });
        }
        validate_prepared_closure(&lock, &dispositions, &manifests, &prepared)?;
        for manifest in manifests.values() {
            self.lifecycle.validate_manifest(manifest)?;
        }

        let changed_manifests = manifests
            .iter()
            .filter(|(package_id, _)| {
                dispositions.get(*package_id) == Some(&InstallDisposition::Add)
            })
            .map(|(package_id, manifest)| (package_id.clone(), manifest.clone()))
            .collect();
        let capability_generation = self.current_capability_generation().await?;
        let grant_snapshot = self
            .planned_grant_snapshot(package_state_revision(capability_generation)?)
            .await?;
        let generated = install_operation(
            &lock,
            &dispositions,
            &surface_selections,
            &manifests,
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
            changed_manifests,
        )?;
        if let Some(attempt) = download_attempt.take() {
            attempt.finish().await?;
        }
        let pending = self
            .admit_planned_graph_operation_in_memory(pending)
            .await?;
        self.authorization.verify_plan(&pending.envelope)?;
        let apply_time = now_ms()?;
        let maintenance = Arc::new(self.maintenance_lock().acquire_shared().await?);
        self.apply_control_install(
            &lock,
            lock_digest,
            &pending,
            &prepared,
            &dispositions,
            &surface_selections,
            apply_time,
            maintenance,
        )
        .await
    }

    async fn apply_control_install(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
        lock_digest: String,
        pending: &PendingPackageGraphOperation,
        prepared: &[PreparedInstallPackage],
        dispositions: &BTreeMap<String, InstallDisposition>,
        surface_selections: &BTreeMap<String, Vec<PluginSurfaceRef>>,
        _apply_time: u64,
        maintenance: Arc<a3s_use_extension::StateMaintenanceGuard>,
    ) -> UseResult<CognitivePackageInstallResult> {
        let artifact_store = self.registry.paths().artifact_store();
        let artifact_admission = artifact_store.acquire_reference_admission().await?;
        for package in prepared {
            if pending.manifests.get(&package.manifest.package_id) != Some(&package.manifest) {
                return Err(package_manager_error(
                    "use.plugin.package_changed",
                    format!(
                        "Prepared package '{}' no longer matches its pending admitted manifest.",
                        package.manifest.package_id
                    ),
                ));
            }
            artifact_store
                .admit_prepared_package(&artifact_admission, &package.package)
                .await?;
        }
        // Release reachability admission before Control drain. Effect owners
        // acquire their own shared package leases; holding admission across
        // drain nested-locks the same reachability file and deadlocks on Windows.
        drop(artifact_admission);
        // Managed factories that configured RuntimeProviderSelection supply
        // deterministic Tool/MCP plan publications; standalone/skill-only stay empty.
        let publications = self.lifecycle.runtime_plan_publications()?;
        super::control_authority::require_control_runtime_readiness_for_publications(
            self.lifecycle.control_runtime_readiness().as_ref(),
            &publications,
        )?;
        let control = self.ensure_control().await?;
        let _snapshot = super::control_authority::apply_pending_through_control(
            control,
            pending,
            &publications,
            maintenance,
        )
        .await?;

        let mut installed_by_id = BTreeMap::new();
        for package in prepared {
            let package_id = package.manifest.package_id.clone();
            let generation = *pending.generations.get(&package_id).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A prepared package has no retained lifecycle generation.",
                )
            })?;
            let selected_surfaces =
                surface_selections
                    .get(&package_id)
                    .cloned()
                    .ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "A prepared package omitted its selected surfaces.",
                        )
                    })?;
            let package_root =
                artifact_store.expanded_package_path(package.package.package_digest())?;
            installed_by_id.insert(
                package_id,
                installed_extension_from_prepared(
                    self.scope(),
                    package,
                    generation,
                    selected_surfaces,
                    package_root,
                    true,
                ),
            );
        }
        let root = installed_by_id
            .get(&lock.root_package_id)
            .cloned()
            .ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The Control-installed cognitive-package root is missing after commit.",
                )
            })?;
        let installed_packages = lock
            .install_order()?
            .into_iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&InstallDisposition::Add)
            })
            .map(|package| package.package_id().to_string())
            .collect();
        let retained_packages = lock
            .install_order()?
            .into_iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&InstallDisposition::Retain)
            })
            .map(|package| package.package_id().to_string())
            .collect();
        Ok(CognitivePackageInstallResult {
            changed: true,
            root,
            package_lock: lock.clone(),
            package_lock_digest: lock_digest,
            plan: Some(pending.envelope.clone()),
            installed_packages,
            retained_packages,
        })
    }

    async fn install_dispositions(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
    ) -> UseResult<(
        BTreeMap<String, InstallDisposition>,
        BTreeMap<String, InstalledExtension>,
    )> {
        self.install_dispositions_control(lock).await
    }

    async fn install_dispositions_control(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
    ) -> UseResult<(
        BTreeMap<String, InstallDisposition>,
        BTreeMap<String, InstalledExtension>,
    )> {
        let control = self.ensure_control().await?;
        let snapshot = control.current_snapshot().await?;
        let mut dispositions = BTreeMap::new();
        let mut installed = BTreeMap::new();
        for package in &lock.packages {
            let selection = snapshot
                .as_ref()
                .and_then(|snapshot| snapshot.package_selection(package.package_id()));
            let disposition = match selection {
                None => InstallDisposition::Add,
                Some(selection) => {
                    if &selection.package != package {
                        return Err(package_manager_error(
                            "use.plugin.package_generation_retirement_required",
                            format!(
                                "Installed package '{}' differs from the resolved dependency lock and must be retired by an explicit upgrade plan.",
                                package.package_id()
                            ),
                        ));
                    }
                    let extension = InstalledExtension {
                        receipt: ExtensionReceipt {
                            schema_version: EXTENSION_RECEIPT_SCHEMA_VERSION,
                            installation: self.scope().clone(),
                            package_id: package.package_id().to_string(),
                            component_id: format!("use/{}", package.package_id()),
                            route_alias: None,
                            version: package.version().to_string(),
                            package_root: std::path::PathBuf::new(),
                            manifest_sha256: package
                                .catalog
                                .record
                                .package
                                .manifest_sha256
                                .clone()
                                .unwrap_or_default(),
                            package_sha256: package.catalog.record.package.sha256.clone(),
                            trust: ExtensionTrust::RegistryTuf,
                            registry: None,
                            verified_catalog: Some(package.catalog.clone()),
                            planning_bundle: None,
                            selected_surfaces: selection.selected_surfaces.clone(),
                            installed_at_unix: 1,
                            enabled: selection.enabled,
                            lifecycle_generation: Some(selection.state_generation),
                        },
                        manifest: ExtensionManifest {
                            schema_version: 3,
                            package_id: package.package_id().to_string(),
                            version: package.version().to_string(),
                            route_alias: None,
                            requires_use: None,
                            dependencies: Vec::new(),
                            repository: None,
                            actions: Vec::new(),
                            tools: Vec::new(),
                            mcp_servers: Vec::new(),
                            okf: Vec::new(),
                            flows: Vec::new(),
                            skills: Vec::new(),
                            ui: Vec::new(),
                        },
                    };
                    let disposition = if selection.enabled {
                        InstallDisposition::Retain
                    } else {
                        InstallDisposition::Add
                    };
                    installed.insert(package.package_id().to_string(), extension);
                    disposition
                }
            };
            dispositions.insert(package.package_id().to_string(), disposition);
        }
        Ok((dispositions, installed))
    }

    async fn verify_published_closure(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
        installed: &BTreeMap<String, InstalledExtension>,
    ) -> UseResult<()> {
        let control = self.ensure_control().await?;
        let snapshot = control.current_snapshot().await?.ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_reconcile_required",
                "Control Store has no committed installation snapshot for the retained closure.",
            )
        })?;
        for package in &lock.packages {
            let extension = installed.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    "A retained dependency is missing from the installed closure.",
                )
            })?;
            let published = snapshot.packages.iter().any(|selection| {
                selection.package.package_id() == extension.receipt.package_id
                    && selection.enabled
                    && Some(selection.state_generation) == extension.receipt.lifecycle_generation
                    && selection.package.catalog.record.package.sha256
                        == extension.receipt.package_sha256
                    && selection
                        .package
                        .catalog
                        .record
                        .package
                        .manifest_sha256
                        .as_deref()
                        == Some(extension.receipt.manifest_sha256.as_str())
            });
            if !published {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Retained package '{}' is not part of the Control installation snapshot.",
                        package.package_id()
                    ),
                ));
            }
        }
        Ok(())
    }
}

fn validate_prepared_closure(
    lock: &a3s_use_core::PluginPackageLock,
    dispositions: &BTreeMap<String, InstallDisposition>,
    manifests: &BTreeMap<String, ExtensionManifest>,
    prepared: &[PreparedInstallPackage],
) -> UseResult<()> {
    let expected = dispositions
        .iter()
        .filter_map(|(package_id, disposition)| {
            (*disposition == InstallDisposition::Add).then_some(package_id.as_str())
        })
        .collect::<BTreeSet<_>>();
    let actual = prepared
        .iter()
        .map(|candidate| candidate.manifest.package_id.as_str())
        .collect::<BTreeSet<_>>();
    if expected.len() != prepared.len()
        || expected != actual
        || manifests.len() != lock.packages.len()
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "The prepared package set does not equal the changed dependency closure.",
        ));
    }
    Ok(())
}

fn install_surface_selections(
    lock: &a3s_use_core::PluginPackageLock,
    dispositions: &BTreeMap<String, InstallDisposition>,
    installed: &BTreeMap<String, InstalledExtension>,
    requested_root_surfaces: Option<&[PluginSurfaceRef]>,
) -> UseResult<BTreeMap<String, Vec<PluginSurfaceRef>>> {
    lock.packages
        .iter()
        .map(|package| {
            let selected = match dispositions.get(package.package_id()) {
                Some(InstallDisposition::Retain) => installed
                    .get(package.package_id())
                    .ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "A retained package is missing its installed surface evidence.",
                        )
                    })?
                    .receipt
                    .selected_surfaces
                    .clone(),
                Some(InstallDisposition::Add) => {
                    let requested = requested_root_surfaces
                        .filter(|_| package.package_id() == lock.root_package_id)
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
                None => {
                    return Err(package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A resolved package has no install disposition.",
                    ))
                }
            };
            Ok((package.package_id().to_string(), selected))
        })
        .collect()
}

fn installed_extension_from_prepared(
    installation: &PlanScope,
    prepared: &PreparedInstallPackage,
    generation: u64,
    selected_surfaces: Vec<PluginSurfaceRef>,
    package_root: std::path::PathBuf,
    enabled: bool,
) -> InstalledExtension {
    InstalledExtension {
        receipt: ExtensionReceipt {
            schema_version: EXTENSION_RECEIPT_SCHEMA_VERSION,
            installation: installation.clone(),
            package_id: prepared.manifest.package_id.clone(),
            component_id: format!("use/{}", prepared.manifest.package_id),
            route_alias: prepared.manifest.route_alias.clone(),
            version: prepared.manifest.version.clone(),
            package_root,
            manifest_sha256: prepared.package.manifest_digest().to_string(),
            package_sha256: Some(prepared.package.package_digest().to_string()),
            trust: prepared.package.trust(),
            registry: prepared.package.registry().cloned(),
            verified_catalog: prepared.package.verified_catalog().cloned(),
            planning_bundle: prepared.package.planning_bundle().cloned(),
            selected_surfaces,
            installed_at_unix: 1,
            enabled,
            lifecycle_generation: Some(generation),
        },
        manifest: prepared.manifest.clone(),
    }
}

pub(crate) fn verify_expected_lock(actual: &str, expected: Option<&str>) -> UseResult<()> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let expected = expected.strip_prefix("sha256:").unwrap_or(expected);
    let actual_value = actual.strip_prefix("sha256:").unwrap_or(actual);
    if expected.len() == 64
        && expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        && expected == actual_value
    {
        return Ok(());
    }
    Err(package_manager_error(
        "use.plugin.package_lock_mismatch",
        "The resolved cognitive-package dependency lock changed after review.",
    )
    .with_detail("expected", expected)
    .with_detail("actual", actual))
}
