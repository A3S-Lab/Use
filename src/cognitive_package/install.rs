use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{PluginOperationAction, PluginPackageLockHost, PluginReleaseChannel, UseResult};
use a3s_use_extension::{
    download_selected_locked_remote_packages, resolve_remote_package_lock,
    ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionManifest, InstalledExtension,
    TrustedRegistry,
};

use crate::plugin_lifecycle::{
    ExtensionGraphCapabilityLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
    PluginLifecycleIntentSpec, PluginPackageGraphLifecycleCoordinator, PluginPackageLifecycleUnit,
};

use super::grant::authorize_planned_operation;
use super::plan::{
    install_generations, install_operation, install_plan_packages, now_ms, package_state_revision,
    static_provider_evidence,
};
use super::store::PendingPackageGraphOperation;
use super::{
    current_host_target, installed_matches_lock, package_manager_error,
    CognitivePackageInstallResult, CognitivePackageManager, InstallDisposition,
};

struct PreparedInstallPackage {
    package: ExtensionLifecyclePackage,
    manifest: ExtensionManifest,
}

impl CognitivePackageManager {
    /// Resolve, revalidate, download, prepare, and atomically publish one
    /// complete schema-v3 dependency closure. The root Registry and enabled
    /// dependency Registries are supplied by the host and remain replaceable.
    #[allow(clippy::too_many_arguments)]
    pub async fn install_remote(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<CognitivePackageInstallResult> {
        let lock = resolve_remote_package_lock(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            PluginPackageLockHost::new(current_host_target()?, env!("CARGO_PKG_VERSION"))?,
        )
        .await?;
        let lock_digest = lock.descriptor_digest()?;
        verify_expected_lock(&lock_digest, expected_package_lock_digest)?;
        let (dispositions, installed) = self.install_dispositions(&lock).await?;

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
            let pending_store = self.pending_store();
            let pending = pending_store
                .get(PluginOperationAction::Install, &lock.root_package_id)
                .await?;
            if let Some(pending) = &pending {
                self.replay_published_install(&lock, pending, &installed)
                    .await?;
            }
            self.graph_store().put(&lock, now_ms()?).await?;
            if let Some(pending) = &pending {
                pending_store.remove(pending).await?;
            }
            let root = installed
                .get(&lock.root_package_id)
                .cloned()
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "The retained root package disappeared during graph adoption.",
                    )
                })?;
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
        let selected_downloads = dispositions
            .iter()
            .filter_map(|(package_id, disposition)| {
                (*disposition == InstallDisposition::Add).then_some(package_id.clone())
            })
            .collect();
        let downloads =
            download_selected_locked_remote_packages(&lock, &registries, &selected_downloads)
                .await?;
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
        let pending_store = self.pending_store();
        let pending = match pending_store
            .get(PluginOperationAction::Install, &lock.root_package_id)
            .await?
        {
            Some(pending) => {
                validate_replay(
                    &pending,
                    &lock,
                    &dispositions,
                    &manifests,
                    &changed_manifests,
                    &self.scope_id,
                )?;
                pending
            }
            None => {
                let snapshot = self.registry.snapshot().await?;
                let grant_snapshot = self
                    .grant_store()
                    .snapshot_scope(&self.scope_id, package_state_revision(snapshot.generation)?)
                    .await?;
                let generated = install_operation(
                    &lock,
                    &dispositions,
                    &manifests,
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
                let generated = PendingPackageGraphOperation::new(
                    generated.envelope,
                    admitted_at_ms,
                    authorization,
                    generated.generations,
                    changed_manifests,
                )?;
                pending_store.put(&generated).await?;
                generated
            }
        };
        if pending.requires_authority_revalidation() {
            self.authorization
                .verify_authority(&pending.envelope.plan)?;
        }
        let apply_time = now_ms()?;

        let mut units = Vec::with_capacity(prepared.len());
        for prepared in prepared {
            let package_id = prepared.manifest.package_id.clone();
            if pending.manifests.get(&package_id) != Some(&prepared.manifest) {
                return Err(package_manager_error(
                    "use.plugin.package_changed",
                    format!(
                        "Prepared package '{}' no longer matches its pending admitted manifest.",
                        package_id
                    ),
                ));
            }
            let generation = *pending.generations.get(&package_id).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A prepared package has no retained lifecycle generation.",
                )
            })?;
            if let Some(current) = installed.get(&package_id) {
                if current.receipt.lifecycle_generation != Some(generation) {
                    return Err(package_manager_error(
                        "use.plugin.package_generation_changed",
                        format!(
                            "Prepared package '{}' no longer matches its pending lifecycle generation.",
                            package_id
                        ),
                    ));
                }
            }
            let identity = ExtensionLifecycleIdentity::new(
                &package_id,
                prepared.package.package_digest(),
                prepared.package.manifest_digest(),
                generation,
            )?;
            let package_root = self.registry.lifecycle_package_root(&identity);
            let intent = PluginLifecycleIntent::from_manifest(
                PluginLifecycleIntentSpec {
                    operation_id: pending.envelope.plan.operation_id.clone(),
                    plan_digest: pending.envelope.plan_digest.clone(),
                    scope_id: self.scope_id.clone(),
                    package_id: package_id.clone(),
                    package_digest: prepared.package.package_digest().to_string(),
                    manifest_digest: prepared.package.manifest_digest().to_string(),
                    generation,
                    action: PluginLifecycleAction::Install,
                },
                &prepared.manifest,
            )?;
            let coordinator = self.lifecycle.install_coordinator(
                self.registry.clone(),
                prepared.package,
                package_root,
            )?;
            units.push(PluginPackageLifecycleUnit::new(
                coordinator,
                intent,
                prepared.manifest,
            )?);
        }

        let graph = PluginPackageGraphLifecycleCoordinator::new(std::sync::Arc::new(
            ExtensionGraphCapabilityLifecycleHost::new(self.registry.clone()),
        ));
        match pending
            .authorization
            .lifecycle_unit(self.grant_store(), &pending.envelope)?
        {
            Some(grants) => {
                graph
                    .apply_install_with_grants(&pending.envelope, &units, &grants, || {
                        now_ms().unwrap_or(apply_time)
                    })
                    .await?;
            }
            None => {
                graph
                    .apply_install(&pending.envelope, &units, || now_ms().unwrap_or(apply_time))
                    .await?;
            }
        }
        self.graph_store().put(&lock, now_ms()?).await?;
        pending_store.remove(&pending).await?;
        let root = self
            .registry
            .get(&lock.root_package_id)
            .await?
            .ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "The published cognitive-package root is missing after graph cutover.",
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
            package_lock: lock,
            package_lock_digest: lock_digest,
            plan: Some(pending.envelope),
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
        let mut dispositions = BTreeMap::new();
        let mut installed = BTreeMap::new();
        for package in &lock.packages {
            let disposition = match self.registry.get(package.package_id()).await? {
                None => InstallDisposition::Add,
                Some(extension) => {
                    if !installed_matches_lock(&extension, &package.catalog)? {
                        return Err(package_manager_error(
                            "use.plugin.package_generation_retirement_required",
                            format!(
                                "Installed package '{}' differs from the resolved dependency lock and must be retired by an explicit upgrade plan.",
                                package.package_id()
                            ),
                        ));
                    }
                    let disposition = if extension.receipt.enabled {
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
        let snapshot = self.registry.snapshot().await?;
        for package in &lock.packages {
            let extension = installed.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    "A retained dependency is missing from the installed closure.",
                )
            })?;
            let published = snapshot.routes.iter().any(|route| {
                route.package_id == extension.receipt.package_id
                    && route.enabled
                    && route.lifecycle_generation == extension.receipt.lifecycle_generation
                    && route.package_sha256 == extension.receipt.package_sha256
                    && route.manifest_sha256 == extension.receipt.manifest_sha256
            });
            if !published {
                return Err(package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    format!(
                        "Retained package '{}' is not part of the published capability generation.",
                        package.package_id()
                    ),
                ));
            }
        }
        Ok(())
    }

    async fn replay_published_install(
        &self,
        lock: &a3s_use_core::PluginPackageLock,
        pending: &PendingPackageGraphOperation,
        installed: &BTreeMap<String, InstalledExtension>,
    ) -> UseResult<()> {
        pending.validate()?;
        if pending.requires_authority_revalidation() {
            self.authorization
                .verify_authority(&pending.envelope.plan)?;
        }
        if pending.envelope.plan.action != PluginOperationAction::Install
            || pending.envelope.package_lock.as_ref() != Some(lock)
            || pending.envelope.plan.scope.id != self.scope_id
        {
            return Err(package_manager_error(
                "use.plugin.package_graph_busy",
                "A published package graph has unrelated pending install evidence.",
            ));
        }

        let mut units = Vec::with_capacity(pending.generations.len());
        for package in lock.install_order()? {
            let Some(generation) = pending.generations.get(package.package_id()).copied() else {
                continue;
            };
            let manifest = pending.manifests.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A pending published package has no admitted manifest.",
                )
            })?;
            self.lifecycle.validate_manifest(manifest)?;
            let extension = installed.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_reconcile_required",
                    "A pending published package is absent from the installed closure.",
                )
            })?;
            if extension.manifest != *manifest
                || extension.receipt.lifecycle_generation != Some(generation)
            {
                return Err(package_manager_error(
                    "use.plugin.package_generation_changed",
                    format!(
                        "Published package '{}' no longer matches its pending lifecycle generation.",
                        package.package_id()
                    ),
                ));
            }
            let state = package
                .catalog
                .selected_state(&super::all_catalog_surfaces(package))?;
            let identity = ExtensionLifecycleIdentity::new(
                package.package_id(),
                state.release.package_sha256.clone(),
                state.release.manifest_sha256.clone(),
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
                    action: PluginLifecycleAction::Install,
                },
                manifest,
            )?;
            let package_root = self.registry.lifecycle_package_root(&identity);
            units.push(PluginPackageLifecycleUnit::new(
                self.lifecycle
                    .published_install_coordinator(self.registry.clone(), package_root)?,
                intent,
                manifest.clone(),
            )?);
        }

        let completed_at_ms = now_ms()?;
        let graph = PluginPackageGraphLifecycleCoordinator::new(std::sync::Arc::new(
            ExtensionGraphCapabilityLifecycleHost::new(self.registry.clone()),
        ));
        match pending
            .authorization
            .lifecycle_unit(self.grant_store(), &pending.envelope)?
        {
            Some(grants) => {
                graph
                    .apply_install_with_grants(&pending.envelope, &units, &grants, || {
                        now_ms().unwrap_or(completed_at_ms)
                    })
                    .await?;
            }
            None => {
                graph
                    .apply_install(&pending.envelope, &units, || {
                        now_ms().unwrap_or(completed_at_ms)
                    })
                    .await?;
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

fn validate_replay(
    pending: &PendingPackageGraphOperation,
    lock: &a3s_use_core::PluginPackageLock,
    dispositions: &BTreeMap<String, InstallDisposition>,
    admitted_manifests: &BTreeMap<String, ExtensionManifest>,
    changed_manifests: &BTreeMap<String, ExtensionManifest>,
    scope_id: &str,
) -> UseResult<()> {
    pending.validate()?;
    let expected_packages = install_plan_packages(lock, dispositions)?;
    let expected_providers = static_provider_evidence(&lock.packages, admitted_manifests)?;
    let state_revision = package_state_revision(pending.envelope.plan.state.capability_generation)?;
    let expected_generations = install_generations(lock, dispositions, state_revision)?;
    if pending.envelope.package_lock.as_ref() != Some(lock)
        || pending.envelope.plan.action != PluginOperationAction::Install
        || pending.envelope.plan.package_id != lock.root_package_id
        || pending.envelope.plan.scope.id != scope_id
        || pending.envelope.plan.state.state_revision != state_revision
        || pending.envelope.plan.packages != expected_packages
        || pending.envelope.plan.providers != expected_providers
        || pending.generations != expected_generations
        || pending.manifests != *changed_manifests
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_busy",
            "The pending cognitive-package install no longer matches the resolved graph.",
        ));
    }
    Ok(())
}

pub(super) fn verify_expected_lock(actual: &str, expected: Option<&str>) -> UseResult<()> {
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
