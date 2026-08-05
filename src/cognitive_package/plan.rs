use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    LockedPluginPackage, PlanAuthority, PlanEnforcementProfile, PlanPackageChangeKind,
    PlanPackageRole, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind, PlannedOperationImpact,
    PlannedPackageTransition, PlannedProviderEvidence, PlannedStateEvidence, PluginOperationAction,
    PluginOperationPlanBinding, PluginOperationPlanDraft, PluginOperationPlanEnvelope,
    PluginPackageLock, PluginSurfaceKind, PluginWorkspaceGrantSnapshot, UseResult,
};
use a3s_use_extension::{ExtensionManifest, PluginMcpLaunch, ToolTaskSource, ToolWorkload};
use sha2::{Digest, Sha256};

use super::{
    all_catalog_surfaces, current_host_target,
    grant::{plan_workspace_grants, PlannedWorkspaceGrantOperation},
    package_manager_error, CognitivePackageAuthorizationProvider, InstallDisposition,
    UninstallDisposition, UpgradeDisposition,
};

const PLAN_LIFETIME_MS: u64 = 60 * 60 * 1000;

pub(super) struct PlannedGraphOperation {
    pub envelope: PluginOperationPlanEnvelope,
    pub generations: BTreeMap<String, u64>,
    pub grants: Option<PlannedWorkspaceGrantOperation>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn install_operation(
    lock: &PluginPackageLock,
    dispositions: &BTreeMap<String, InstallDisposition>,
    manifests: &BTreeMap<String, ExtensionManifest>,
    registry_generation: u64,
    scope_id: &str,
    created_at_ms: u64,
    grant_snapshot: &PluginWorkspaceGrantSnapshot,
    authorization: &dyn CognitivePackageAuthorizationProvider,
) -> UseResult<PlannedGraphOperation> {
    let packages = install_plan_packages(lock, dispositions)?;
    if dispositions.get(&lock.root_package_id) != Some(&InstallDisposition::Add) {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "A new graph install must add its root package generation.",
        ));
    }

    let state_revision = package_state_revision(registry_generation)?;
    let lock_digest = lock.descriptor_digest()?;
    let providers = static_provider_evidence(&lock.packages, manifests)?;
    let impact = PlannedOperationImpact {
        download_bytes: lock
            .packages
            .iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&InstallDisposition::Add)
            })
            .map(|package| package.catalog.record.archive.length)
            .sum(),
        installed_bytes_after: lock
            .packages
            .iter()
            .map(|package| package.catalog.record.package.expanded_bytes)
            .sum(),
        reclaimed_bytes: 0,
        drain_required: false,
        retained_data: false,
        okf_changes: Vec::new(),
    };
    let mut draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Install,
        lock.root_package_id.clone(),
        format!("use/{}", lock.root_package_id),
        packages,
        providers,
        Vec::new(),
        impact,
        PlannedStateEvidence {
            state_revision,
            capability_generation: registry_generation,
            receipt_digest: None,
        },
    )?;
    let binding = binding(
        PluginOperationAction::Install,
        &lock_digest,
        scope_id,
        created_at_ms,
        authorization.bind_authority(&draft)?,
    )?;
    let grants = plan_workspace_grants(&mut draft, &binding, grant_snapshot, false, true)?;
    let plan = draft.bind(binding)?;
    let envelope = PluginOperationPlanEnvelope::new_with_package_lock(plan, lock.clone())?;
    let generations = install_generations(lock, dispositions, state_revision)?;
    Ok(PlannedGraphOperation {
        envelope,
        generations,
        grants,
    })
}

pub(super) fn install_plan_packages(
    lock: &PluginPackageLock,
    dispositions: &BTreeMap<String, InstallDisposition>,
) -> UseResult<Vec<PlannedPackageTransition>> {
    let mut packages = Vec::with_capacity(lock.packages.len());
    for package in &lock.packages {
        let role = package_role(lock, package.package_id());
        let surfaces = all_catalog_surfaces(package);
        let disposition = dispositions.get(package.package_id()).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "A resolved package has no install disposition.",
            )
        })?;
        let transition = match disposition {
            InstallDisposition::Add => package.catalog.install_transition(role, &surfaces)?,
            InstallDisposition::Retain => {
                let state = package.catalog.selected_state(&surfaces)?;
                PlannedPackageTransition::resolved(
                    package.package_id(),
                    role,
                    PlanPackageChangeKind::Retain,
                    Some(state.clone()),
                    Some(state),
                    None,
                )?
            }
        };
        packages.push(transition);
    }
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    Ok(packages)
}

pub(super) fn install_generations(
    lock: &PluginPackageLock,
    dispositions: &BTreeMap<String, InstallDisposition>,
    state_revision: u64,
) -> UseResult<BTreeMap<String, u64>> {
    let mut generations = BTreeMap::new();
    for (index, package) in lock.install_order()?.into_iter().enumerate() {
        if dispositions.get(package.package_id()) != Some(&InstallDisposition::Add) {
            continue;
        }
        let offset = u64::try_from(index).map_err(|_| {
            package_manager_error(
                "use.plugin.package_generation_exhausted",
                "The dependency graph generation offset is too large.",
            )
        })?;
        let generation = state_revision.checked_add(offset).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_generation_exhausted",
                "The package lifecycle generation counter is exhausted.",
            )
        })?;
        generations.insert(package.package_id().to_string(), generation);
    }
    Ok(generations)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn uninstall_operation(
    lock: &PluginPackageLock,
    dispositions: &BTreeMap<String, UninstallDisposition>,
    generations: BTreeMap<String, u64>,
    root_receipt_digest: String,
    registry_generation: u64,
    scope_id: &str,
    created_at_ms: u64,
    grant_snapshot: &PluginWorkspaceGrantSnapshot,
    authorization: &dyn CognitivePackageAuthorizationProvider,
) -> UseResult<PlannedGraphOperation> {
    let mut packages = Vec::with_capacity(lock.packages.len());
    for package in &lock.packages {
        let role = package_role(lock, package.package_id());
        let surfaces = all_catalog_surfaces(package);
        let state = package.catalog.selected_state(&surfaces)?;
        let transition = match dispositions.get(package.package_id()) {
            Some(UninstallDisposition::Remove) => PlannedPackageTransition::resolved(
                package.package_id(),
                role,
                PlanPackageChangeKind::Remove,
                Some(state),
                None,
                None,
            )?,
            Some(UninstallDisposition::Retain) => PlannedPackageTransition::resolved(
                package.package_id(),
                role,
                PlanPackageChangeKind::Retain,
                Some(state.clone()),
                Some(state),
                None,
            )?,
            None => {
                return Err(package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A locked package has no uninstall disposition.",
                ))
            }
        };
        packages.push(transition);
    }
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    if dispositions.get(&lock.root_package_id) != Some(&UninstallDisposition::Remove) {
        return Err(package_manager_error(
            "use.plugin.package_has_dependents",
            format!(
                "Cognitive package '{}' is still required by another installed root.",
                lock.root_package_id
            ),
        ));
    }
    let state_revision = package_state_revision(registry_generation)?;
    let lock_digest = lock.descriptor_digest()?;
    let impact = PlannedOperationImpact {
        download_bytes: 0,
        installed_bytes_after: 0,
        reclaimed_bytes: lock
            .packages
            .iter()
            .filter(|package| {
                dispositions.get(package.package_id()) == Some(&UninstallDisposition::Remove)
            })
            .map(|package| package.catalog.record.package.expanded_bytes)
            .sum(),
        drain_required: false,
        retained_data: true,
        okf_changes: Vec::new(),
    };
    let mut draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Uninstall,
        lock.root_package_id.clone(),
        format!("use/{}", lock.root_package_id),
        packages,
        Vec::new(),
        Vec::new(),
        impact,
        PlannedStateEvidence {
            state_revision,
            capability_generation: registry_generation,
            receipt_digest: Some(root_receipt_digest),
        },
    )?;
    let binding = binding(
        PluginOperationAction::Uninstall,
        &lock_digest,
        scope_id,
        created_at_ms,
        authorization.bind_authority(&draft)?,
    )?;
    let grants = plan_workspace_grants(&mut draft, &binding, grant_snapshot, true, false)?;
    let plan = draft.bind(binding)?;
    Ok(PlannedGraphOperation {
        envelope: PluginOperationPlanEnvelope::new_with_package_lock(plan, lock.clone())?,
        generations,
        grants,
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn upgrade_operation(
    candidate_lock: &PluginPackageLock,
    prior_lock: &PluginPackageLock,
    dispositions: &BTreeMap<String, UpgradeDisposition>,
    manifests: &BTreeMap<String, ExtensionManifest>,
    prior_generations: &BTreeMap<String, u64>,
    root_receipt_digest: String,
    registry_generation: u64,
    scope_id: &str,
    created_at_ms: u64,
    grant_snapshot: &PluginWorkspaceGrantSnapshot,
    authorization: &dyn CognitivePackageAuthorizationProvider,
) -> UseResult<PlannedGraphOperation> {
    if candidate_lock.root_package_id != prior_lock.root_package_id
        || candidate_lock.host != prior_lock.host
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "Prior and candidate cognitive-package locks belong to different roots or hosts.",
        ));
    }
    let package_ids = prior_lock
        .packages
        .iter()
        .chain(&candidate_lock.packages)
        .map(|package| package.package_id())
        .collect::<std::collections::BTreeSet<_>>();
    if package_ids.len() != dispositions.len()
        || package_ids
            .iter()
            .any(|package_id| !dispositions.contains_key(*package_id))
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_invalid",
            "Upgrade dispositions do not cover the exact prior/candidate lock union.",
        ));
    }

    let mut packages = Vec::with_capacity(dispositions.len());
    for (package_id, disposition) in dispositions {
        let candidate = candidate_lock.package(package_id);
        let prior = prior_lock.package(package_id);
        let role = package_role(candidate_lock, package_id);
        let transition = match disposition {
            UpgradeDisposition::Add => {
                let candidate = candidate.ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "An added package is absent from the candidate lock.",
                    )
                })?;
                let surfaces = all_catalog_surfaces(candidate);
                candidate.catalog.install_transition(role, &surfaces)?
            }
            UpgradeDisposition::Replace => {
                let candidate = candidate.ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A replacement package is absent from the candidate lock.",
                    )
                })?;
                let prior = prior.ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A replacement candidate has no exact prior lock node.",
                    )
                })?;
                let surfaces = all_catalog_surfaces(candidate);
                candidate.catalog.replace_transition(
                    &prior.catalog,
                    role,
                    &all_catalog_surfaces(prior),
                    &surfaces,
                )?
            }
            UpgradeDisposition::Remove => {
                let prior = prior.ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A removed package is absent from the prior lock.",
                    )
                })?;
                if candidate.is_some() {
                    return Err(package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A removed package is still present in the candidate lock.",
                    ));
                }
                prior
                    .catalog
                    .remove_transition(role, &all_catalog_surfaces(prior))?
            }
            UpgradeDisposition::Retain => {
                let retained = prior.or(candidate).ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_graph_invalid",
                        "A retained package is absent from both exact dependency locks.",
                    )
                })?;
                let before = retained
                    .catalog
                    .selected_state(&all_catalog_surfaces(retained))?;
                if let (Some(prior), Some(candidate)) = (prior, candidate) {
                    let after = candidate
                        .catalog
                        .selected_state(&all_catalog_surfaces(candidate))?;
                    if before != after || prior.catalog != candidate.catalog {
                        return Err(package_manager_error(
                            "use.plugin.package_graph_invalid",
                            "A retained package changed its exact catalog or selected surface state.",
                        ));
                    }
                }
                PlannedPackageTransition::resolved(
                    package_id,
                    role,
                    PlanPackageChangeKind::Retain,
                    Some(before.clone()),
                    Some(before),
                    None,
                )?
            }
        };
        packages.push(transition);
    }
    packages.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    if dispositions
        .values()
        .all(|disposition| *disposition == UpgradeDisposition::Retain)
    {
        return Err(package_manager_error(
            "use.plugin.package_graph_unchanged",
            "The resolved cognitive-package graph has no upgrade transition.",
        ));
    }

    let drain_required = packages.iter().any(|package| {
        matches!(
            package.change,
            PlanPackageChangeKind::Replace | PlanPackageChangeKind::Remove
        ) && package.before.as_ref().is_some_and(|state| {
            state
                .permissions
                .surfaces
                .iter()
                .any(|permission| permission.private_service)
        })
    });

    let state_revision = package_state_revision(registry_generation)?;
    let lock_digest = candidate_lock.descriptor_digest()?;
    let upgrade_identity_digest = digest(&format!(
        "a3s-use-package-graph-upgrade-v1\n{}\n{}",
        prior_lock.descriptor_digest()?,
        lock_digest
    ));
    let provider_packages =
        candidate_lock
            .packages
            .iter()
            .chain(prior_lock.packages.iter().filter(|package| {
                candidate_lock.package(package.package_id()).is_none()
                    && dispositions.get(package.package_id()) == Some(&UpgradeDisposition::Retain)
            }));
    let providers = static_provider_evidence(provider_packages, manifests)?;
    let impact = PlannedOperationImpact {
        download_bytes: candidate_lock
            .packages
            .iter()
            .filter(|package| {
                matches!(
                    dispositions.get(package.package_id()),
                    Some(UpgradeDisposition::Add | UpgradeDisposition::Replace)
                )
            })
            .map(|package| package.catalog.record.archive.length)
            .sum(),
        installed_bytes_after: candidate_lock
            .packages
            .iter()
            .map(|package| package.catalog.record.package.expanded_bytes)
            .sum(),
        reclaimed_bytes: prior_lock
            .packages
            .iter()
            .filter(|package| {
                matches!(
                    dispositions.get(package.package_id()),
                    Some(UpgradeDisposition::Replace | UpgradeDisposition::Remove)
                )
            })
            .map(|package| package.catalog.record.package.expanded_bytes)
            .sum(),
        drain_required,
        retained_data: false,
        okf_changes: Vec::new(),
    };
    let mut draft = PluginOperationPlanDraft::new(
        PluginOperationAction::Upgrade,
        candidate_lock.root_package_id.clone(),
        format!("use/{}", candidate_lock.root_package_id),
        packages,
        providers,
        Vec::new(),
        impact,
        PlannedStateEvidence {
            state_revision,
            capability_generation: registry_generation,
            receipt_digest: Some(root_receipt_digest),
        },
    )?;
    let binding = binding(
        PluginOperationAction::Upgrade,
        &upgrade_identity_digest,
        scope_id,
        created_at_ms,
        authorization.bind_authority(&draft)?,
    )?;
    let grants = plan_workspace_grants(&mut draft, &binding, grant_snapshot, true, true)?;
    let plan = draft.bind(binding)?;
    let envelope = PluginOperationPlanEnvelope::new_with_upgrade_package_locks(
        plan,
        prior_lock.clone(),
        candidate_lock.clone(),
    )?;
    let mut generations = BTreeMap::new();
    for (index, package) in candidate_lock.install_order()?.into_iter().enumerate() {
        let disposition = dispositions.get(package.package_id()).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "A candidate package lost its upgrade disposition.",
            )
        })?;
        match disposition {
            UpgradeDisposition::Retain => continue,
            UpgradeDisposition::Remove => {
                return Err(package_manager_error(
                    "use.plugin.package_graph_invalid",
                    "A removed package appeared in the candidate install order.",
                ))
            }
            UpgradeDisposition::Add | UpgradeDisposition::Replace => {}
        }
        let offset = u64::try_from(index).map_err(|_| {
            package_manager_error(
                "use.plugin.package_generation_exhausted",
                "The dependency graph generation offset is too large.",
            )
        })?;
        let mut generation = state_revision.checked_add(offset).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_generation_exhausted",
                "The package lifecycle generation counter is exhausted.",
            )
        })?;
        if *disposition == UpgradeDisposition::Replace {
            let prior = prior_generations.get(package.package_id()).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_generation_changed",
                    "A replacement package omitted its exact prior lifecycle generation.",
                )
            })?;
            generation = generation.max(prior.checked_add(1).ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_generation_exhausted",
                    "A prior package lifecycle generation cannot advance.",
                )
            })?);
        }
        generations.insert(package.package_id().to_string(), generation);
    }
    Ok(PlannedGraphOperation {
        envelope,
        generations,
        grants,
    })
}

pub(super) fn now_ms() -> UseResult<u64> {
    let value = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| {
            package_manager_error(
                "use.plugin.package_clock_invalid",
                "The system clock is before the Unix epoch.",
            )
        })?
        .as_millis();
    u64::try_from(value).map_err(|_| {
        package_manager_error(
            "use.plugin.package_clock_invalid",
            "The system clock exceeds the package-plan time bound.",
        )
    })
}

pub(super) fn package_state_revision(registry_generation: u64) -> UseResult<u64> {
    registry_generation.checked_add(1).ok_or_else(|| {
        package_manager_error(
            "use.plugin.package_generation_exhausted",
            "The package state revision counter is exhausted.",
        )
    })
}

fn binding(
    action: PluginOperationAction,
    lock_digest: &str,
    scope_id: &str,
    created_at_ms: u64,
    authority: PlanAuthority,
) -> UseResult<PluginOperationPlanBinding> {
    let expires_at_ms = created_at_ms.checked_add(PLAN_LIFETIME_MS).ok_or_else(|| {
        package_manager_error(
            "use.plugin.package_clock_invalid",
            "The package-plan expiration time overflowed.",
        )
    })?;
    let operation = match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Uninstall => "uninstall",
        PluginOperationAction::Upgrade => "upgrade",
    };
    let identity = lock_digest.strip_prefix("sha256:").unwrap_or(lock_digest);
    Ok(PluginOperationPlanBinding {
        operation_id: format!("{operation}:package-graph:{identity}"),
        created_at_ms,
        expires_at_ms,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: scope_id.to_string(),
        },
        authority,
    })
}

pub(super) fn static_provider_evidence<'a>(
    packages: impl IntoIterator<Item = &'a LockedPluginPackage>,
    manifests: &BTreeMap<String, ExtensionManifest>,
) -> UseResult<Vec<PlannedProviderEvidence>> {
    let target = current_host_target()?;
    let provider_id = "a3s-use-native-launcher";
    let provider_build_id = format!("a3s-use:{}:{target}", env!("CARGO_PKG_VERSION"));
    let capability_digest = digest(&format!(
        "a3s-use-native-launcher-v1\n{}\n{target}",
        env!("CARGO_PKG_VERSION")
    ));
    let mut providers = Vec::new();
    for package in packages {
        let manifest = manifests.get(package.package_id()).ok_or_else(|| {
            package_manager_error(
                "use.plugin.package_graph_invalid",
                "A locked package has no admitted manifest for provider planning.",
            )
        })?;
        let state = package
            .catalog
            .selected_state(&all_catalog_surfaces(package))?;
        for surface in &state.release.surfaces {
            if !matches!(
                surface.kind,
                PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
            ) {
                continue;
            }
            validate_static_surface(manifest, surface.kind, &surface.id)?;
            let permission = state
                .permissions
                .surfaces
                .iter()
                .find(|permission| permission.surface == surface.reference())
                .ok_or_else(|| {
                    package_manager_error(
                        "use.plugin.package_provider_invalid",
                        "An executable cognitive-package surface omitted its permission ceiling.",
                    )
                })?;
            providers.push(PlannedProviderEvidence {
                surface: PlanQualifiedSurfaceRef {
                    package_id: package.package_id().to_string(),
                    surface: surface.reference(),
                },
                provider_id: provider_id.to_string(),
                provider_build_id: provider_build_id.clone(),
                capability_digest: capability_digest.clone(),
                semantics_profile_digest: digest(&format!(
                    "a3s-use-static-surface-v1\n{}\n{:?}\n{}\n{}",
                    package.package_id(),
                    surface.kind,
                    surface.id,
                    state.release.package_sha256
                )),
                enforcement: if permission.native_execution {
                    PlanEnforcementProfile::NativeUnconfined
                } else {
                    PlanEnforcementProfile::Container
                },
            });
        }
    }
    providers.sort_by(|left, right| left.surface.cmp(&right.surface));
    Ok(providers)
}

fn validate_static_surface(
    manifest: &ExtensionManifest,
    kind: PluginSurfaceKind,
    surface_id: &str,
) -> UseResult<()> {
    let supported = match kind {
        PluginSurfaceKind::Tool => manifest.tools.iter().any(|surface| {
            surface.id == surface_id
                && matches!(
                    &surface.workload,
                    ToolWorkload::Task(task)
                        if matches!(&task.source, ToolTaskSource::Executable { .. })
                )
        }),
        PluginSurfaceKind::Mcp => manifest.mcp_servers.iter().any(|surface| {
            surface.id == surface_id && matches!(surface.launch, PluginMcpLaunch::Stdio { .. })
        }),
        _ => false,
    };
    if supported {
        Ok(())
    } else {
        Err(package_manager_error(
            "use.plugin.runtime_provider_required",
            format!(
                "Executable surface '{surface_id}' requires an explicitly injected Runtime provider."
            ),
        ))
    }
}

fn package_role(lock: &PluginPackageLock, package_id: &str) -> PlanPackageRole {
    if package_id == lock.root_package_id {
        PlanPackageRole::Root
    } else {
        PlanPackageRole::Dependency
    }
}

fn digest(value: &str) -> String {
    format!("sha256:{:x}", Sha256::digest(value.as_bytes()))
}
