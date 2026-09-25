//! Control-authority extension capability projection.
//!
//! Hosted under the capability registry facade; package identity and enablement
//! come only from Control, never from legacy extension receipts.

use std::path::{Path, PathBuf};

use a3s_use_core::{
    InstallationId, InstallationSnapshot, InstalledPluginPlanEvidence, OkfCapabilityProjection,
    PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef, Readiness, UseError, UseResult,
    INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA,
};

use super::lease::CapabilityUpstreamEvidence;
#[cfg(feature = "extensions")]
use super::executable_tools::executable_tool_evidence_from_package;
#[cfg(feature = "extensions")]
use super::managed_mcp::mcp_evidence_from_store;
#[cfg(feature = "extensions")]
use super::runtime_tasks::runtime_task_evidence_from_store;
use super::{
    activity_asset, flow_asset, skill_surface, ActivityBarContribution, CapabilityBinding,
    CapabilityOrigin, CapabilityRegistrySnapshot, ExecutableToolProjection, FlowEngine,
    FlowRuntime, FlowSurface, ManagedAsset, McpLaunchProjection, McpRuntimeProjection,
    McpServerProjection, McpSurface, McpSurfaceActivation, McpTransport, PluginPlannerEvidence,
    ProjectedLifecycleIdentity, RepositoryBinding, SkillSurface, ToolTaskProjection,
    MAX_STABLE_SNAPSHOT_ATTEMPTS, PLANNER_EVIDENCE_SCHEMA_VERSION, UI_DEPENDENCY_EVIDENCE_SCHEMA,
};
#[cfg(feature = "extensions")]
use crate::surface_reconciler::{
    reconcile_with_runtime_and_knowledge, PluginDesiredState, PluginObservedState,
    ReconciledSurface, SurfaceDesiredState, SurfaceObservations, SurfaceObservedState,
    SurfaceOwner, SurfaceReconcileSnapshot, SurfaceStateReason,
};
#[cfg(feature = "extensions")]
use crate::{
    flow_runtime::FlowRuntimeBindingStore,
    okf_knowledge::{
        OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient,
        SqliteOkfKnowledgeAdapter,
    },
    plugin_runtime::RuntimeBindingStore,
};

#[cfg(feature = "extensions")]
pub(crate) async fn stable_extensions(
    registry: &a3s_use_extension::ExtensionRegistry,
) -> UseResult<(CapabilityUpstreamEvidence, Vec<CapabilityBinding>)> {
    // Installation authority is Control-only (legacy leaves fail closed).
    // Never fall back to InstallationSnapshotStore / extensions receipts.
    stable_extensions_from_control(registry).await
}

/// Control-authority capability projection.
///
/// Package-graph enablement and package identity come only from Control.
/// Packages are loaded from the Artifact Store via
/// [`ExtensionRegistry::load_control_package_selection`]; legacy registry
/// receipts under `extensions/` are rejected beside Control.
#[cfg(feature = "extensions")]
async fn stable_extensions_from_control(
    registry: &a3s_use_extension::ExtensionRegistry,
) -> UseResult<(CapabilityUpstreamEvidence, Vec<CapabilityBinding>)> {
    let paths = registry.paths();
    let state_root = paths.installation_state_root();
    if !crate::control_store::control_database_present(&state_root) {
        return Err(UseError::new(
            "use.capability.control_required",
            "Capability projection requires an initialized Control Store.",
        )
        .with_suggestion(
            "Initialize Control Store for this installation before projecting capabilities.",
        ));
    }
    crate::control_store::reject_legacy_authority_paths(&state_root)?;
    for _ in 0..MAX_STABLE_SNAPSHOT_ATTEMPTS {
        let installation_before = crate::control_store::read_current_installation_snapshot(
            &state_root,
            paths.installation(),
        )
        .await?;
        let Some(capabilities) =
            project_control_packages(registry, installation_before.as_ref()).await?
        else {
            continue;
        };
        let installation_after = crate::control_store::read_current_installation_snapshot(
            &state_root,
            paths.installation(),
        )
        .await?;
        if installation_before != installation_after {
            continue;
        }
        // Empty Control uses the same deterministic face as diagnostics — never
        // `registry.json` / published_snapshot.
        let face = a3s_use_extension::ExtensionRegistrySnapshot::empty(
            paths.installation().clone(),
        )?;
        return Ok((
            CapabilityUpstreamEvidence::from_snapshot(&face, installation_before.as_ref())?,
            capabilities,
        ));
    }
    Err(UseError::new(
        "use.capability.registry_busy",
        "The Control installation snapshot changed repeatedly while capabilities were projected.",
    )
    .with_suggestion("Retry the capability snapshot after the current Control operation."))
}

#[cfg(feature = "extensions")]
async fn project_control_packages(
    registry: &a3s_use_extension::ExtensionRegistry,
    installation: Option<&InstallationSnapshot>,
) -> UseResult<Option<Vec<CapabilityBinding>>> {
    let Some(installation) = installation else {
        return Ok(Some(Vec::new()));
    };
    let mut capabilities = Vec::with_capacity(installation.packages.len());
    for selection in &installation.packages {
        let extension = match registry.load_control_package_selection(selection).await {
            Ok(extension) => extension,
            Err(error)
                if error.code == "use.artifact_store.ownership_invalid"
                    || error.code == "use.extension.package_digest_mismatch"
                    || error.code.starts_with("use.extension.") =>
            {
                // Package bytes or catalog binding changed mid-scan; retry.
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        capabilities.push(
            project_extension(&extension, surfaces, selection.enabled, registry.paths()).await?,
        );
    }
    Ok(Some(capabilities))
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn stable_extensions(
    installation: &InstallationId,
) -> UseResult<(CapabilityUpstreamEvidence, Vec<CapabilityBinding>)> {
    Ok((
        CapabilityUpstreamEvidence::empty(installation.clone()),
        Vec::new(),
    ))
}

#[cfg(feature = "extensions")]
async fn project_extensions(
    registry: &a3s_use_extension::ExtensionRegistry,
    snapshot: &a3s_use_extension::ExtensionRegistrySnapshot,
    installation: Option<&InstallationSnapshot>,
) -> UseResult<Option<Vec<CapabilityBinding>>> {
    let mut capabilities = Vec::with_capacity(snapshot.packages.len());
    for binding in &snapshot.packages {
        let Some(extension) = registry.get_snapshot_binding(binding).await? else {
            return Ok(None);
        };
        let receipt = &extension.receipt;
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if receipt.package_id != binding.package_id
            || receipt.component_id != binding.component_id
            || receipt.route_alias != binding.route_alias
            || receipt.version != binding.version
            || receipt.package_root != binding.package_root
            || receipt.manifest_sha256 != binding.manifest_sha256
            || receipt.lifecycle_generation != binding.lifecycle_generation
            || receipt.enabled != binding.enabled
            || surfaces != binding.surfaces
        {
            return Ok(None);
        }
        let desired_enabled = match installation
            .and_then(|snapshot| snapshot.package_selection(&receipt.package_id))
        {
            Some(selection) => {
                if selection.package.catalog != *extension.plan_ready_catalog()?
                    || selection.selected_surfaces != extension.selected_surfaces()?
                {
                    return Err(UseError::new(
                        "use.capability.installation_snapshot_invalid",
                        "A lifecycle-managed package disagrees with its installation selection.",
                    ));
                }
                selection.enabled
            }
            None if receipt.verified_catalog.is_some() => {
                return Err(UseError::new(
                    "use.capability.installation_snapshot_invalid",
                    "A lifecycle-managed capability package is absent from the installation snapshot.",
                ));
            }
            None => receipt.enabled,
        };
        capabilities.push(
            project_extension(&extension, surfaces, desired_enabled, registry.paths()).await?,
        );
    }
    Ok(Some(capabilities))
}

#[cfg(feature = "extensions")]
async fn project_extension(
    extension: &a3s_use_extension::InstalledExtension,
    surfaces: Vec<String>,
    desired_enabled: bool,
    paths: &a3s_use_extension::ExtensionPaths,
) -> UseResult<CapabilityBinding> {
    let scope = paths.installation().clone();
    let flow_observations = flow_observations_from_store(
        extension,
        &FlowRuntimeBindingStore::for_control_authority(paths),
        &scope,
    )
    .await?;
    let knowledge_evidence = knowledge_evidence_from_store(
        extension,
        &OkfKnowledgeBindingStore::for_control_authority(paths),
        &OkfKnowledgeClient::new(std::sync::Arc::new(
            SqliteOkfKnowledgeAdapter::from_extension_paths(paths),
        )),
        &scope,
    )
    .await?;
    let runtime_task_evidence = runtime_task_evidence_from_store(
        extension,
        &RuntimeBindingStore::for_control_authority(paths),
        &scope,
    )
    .await?;
    let executable_tool_evidence = executable_tool_evidence_from_package(extension).await?;
    let mcp_evidence = mcp_evidence_from_store(
        extension,
        &RuntimeBindingStore::for_control_authority(paths),
        &scope,
    )
    .await?;
    let mut host_observations = flow_observations;
    for (surface, state) in runtime_task_evidence
        .observations
        .into_iter()
        .chain(executable_tool_evidence.observations)
        .chain(mcp_evidence.observations)
        .chain(knowledge_evidence.failures)
    {
        if host_observations.insert(surface, state).is_some() {
            return Err(UseError::new(
                "use.capability.host_observation_invalid",
                "Two production hosts reported the same cognitive-package surface.",
            ));
        }
    }
    project_extension_for_host_with_evidence(
        extension,
        surfaces,
        CapabilityHostProjectionContext {
            desired_enabled,
            host_version: env!("CARGO_PKG_VERSION"),
            host_observations: &host_observations,
            knowledge_bindings: &knowledge_evidence.bindings,
            runtime_tasks: &runtime_task_evidence.projections,
            mcp_projections: &mcp_evidence.projections,
            executable_tools: &executable_tool_evidence.projections,
        },
    )
    .await
}

#[cfg(feature = "extensions")]
#[cfg(test)]
pub(crate) async fn project_extension_for_host(
    extension: &a3s_use_extension::InstalledExtension,
    surfaces: Vec<String>,
    host_version: &str,
) -> UseResult<CapabilityBinding> {
    project_extension_for_host_with_evidence(
        extension,
        surfaces,
        CapabilityHostProjectionContext {
            desired_enabled: extension.receipt.enabled,
            host_version,
            host_observations: &SurfaceObservations::new(),
            knowledge_bindings: &[],
            runtime_tasks: &[],
            mcp_projections: &[],
            executable_tools: &[],
        },
    )
    .await
}

#[cfg(feature = "extensions")]
pub(crate) struct CapabilityHostProjectionContext<'a> {
    pub(crate) desired_enabled: bool,
    pub(crate) host_version: &'a str,
    pub(crate) host_observations: &'a SurfaceObservations,
    pub(crate) knowledge_bindings: &'a [OkfKnowledgeBinding],
    pub(crate) runtime_tasks: &'a [ToolTaskProjection],
    pub(crate) mcp_projections: &'a [McpServerProjection],
    pub(crate) executable_tools: &'a [ExecutableToolProjection],
}

#[cfg(feature = "extensions")]
pub(crate) async fn project_extension_for_host_with_evidence(
    extension: &a3s_use_extension::InstalledExtension,
    surfaces: Vec<String>,
    context: CapabilityHostProjectionContext<'_>,
) -> UseResult<CapabilityBinding> {
    let receipt = &extension.receipt;
    let compatible = extension.supports_use_version(context.host_version);
    let observations = surface_observations(
        extension,
        context.desired_enabled && receipt.enabled && compatible,
        context.host_observations,
    )
    .await?;
    let knowledge_observations = context
        .knowledge_bindings
        .iter()
        .map(|binding| (binding.receipt.clone(), binding.observation.clone()))
        .collect::<Vec<_>>();
    let reconciliation = Some(reconcile_with_runtime_and_knowledge(
        &extension.manifest,
        if context.desired_enabled {
            PluginDesiredState::Enabled
        } else {
            PluginDesiredState::InstalledDisabled
        },
        compatible,
        &observations,
        None,
        &knowledge_observations,
    )?);
    let active = context.desired_enabled
        && receipt.enabled
        && compatible
        && reconciliation
            .as_ref()
            .is_some_and(|snapshot| snapshot.capability_ready);
    let readiness = match reconciliation
        .as_ref()
        .expect("reconciliation is present")
        .observed
    {
        PluginObservedState::Ready | PluginObservedState::Degraded => Readiness::Ready,
        PluginObservedState::Broken | PluginObservedState::Incompatible => Readiness::Broken,
        PluginObservedState::Installed
        | PluginObservedState::Reconciling
        | PluginObservedState::Draining
        | PluginObservedState::Removed => Readiness::Unknown,
    };
    let mcp = None;
    let mut mcp_servers = Vec::new();
    let mut skills = Vec::new();
    if active {
        let snapshot = reconciliation.as_ref().expect("reconciliation is present");
        for skill in &extension.manifest.skills {
            if snapshot.publishes(PluginSurfaceKind::Skill, &skill.id) {
                skills
                    .push(skill_surface(&skill.id, receipt.package_root.join(&skill.path)).await?);
            }
        }
    }
    let mut activity_bar = Vec::new();
    let mut flows = Vec::new();
    let mut knowledge = Vec::new();
    let mut tool_tasks = Vec::new();
    let mut executable_tools = Vec::new();
    let surface_graph = if active {
        extension.manifest.plugin_surfaces()?
    } else {
        Vec::new()
    };
    if let Some(snapshot) = reconciliation.as_ref().filter(|_| active) {
        mcp_servers.extend(
            context
                .mcp_projections
                .iter()
                .filter(|projection| snapshot.publishes(PluginSurfaceKind::Mcp, &projection.id))
                .cloned(),
        );
        tool_tasks.extend(
            context
                .runtime_tasks
                .iter()
                .filter(|task| snapshot.publishes(PluginSurfaceKind::Tool, &task.surface_id))
                .cloned(),
        );
        executable_tools.extend(
            context
                .executable_tools
                .iter()
                .filter(|tool| snapshot.publishes(PluginSurfaceKind::Tool, &tool.surface_id))
                .cloned(),
        );
    }
    if let Some(snapshot) = reconciliation.as_ref().filter(|_| active) {
        for surface in &extension.manifest.flows {
            if !snapshot.publishes(PluginSurfaceKind::Flow, &surface.id) {
                continue;
            }
            flows.push(FlowSurface {
                id: surface.id.clone(),
                engine: match surface.engine {
                    a3s_use_extension::PluginFlowEngine::A3sFlow => FlowEngine::A3sFlow,
                },
                runtime: match surface.runtime {
                    a3s_use_extension::PluginFlowRuntime::NativeTs => FlowRuntime::NativeTs,
                },
                source: flow_asset(receipt.package_root.join(&surface.source)).await?,
                export_name: surface.export_name.clone(),
                requires_tools: surface.requires_tools.clone(),
                requires_mcp: surface.requires_mcp.clone(),
                requires_okf: surface.requires_okf.clone(),
            });
        }
    }
    // Knowledge is read-only cited retrieval. Admit a promoted OKF surface when
    // the knowledge-host observation is healthy, even if sibling Tool/MCP/
    // Skill/UI surfaces are still reconciling (AtomicScoped Code exec does not
    // start MCP/Tool, so waiting on capability_ready would strand Knowledge).
    if context.desired_enabled && receipt.enabled && compatible {
        if let Some(snapshot) = reconciliation.as_ref() {
            for binding in context.knowledge_bindings {
                let surface_id = &binding.receipt.surface.surface.id;
                let okf_ready = snapshot.publishes(PluginSurfaceKind::Okf, surface_id)
                    || snapshot.surfaces.iter().any(|surface| {
                        surface.surface.kind == PluginSurfaceKind::Okf
                            && surface.surface.id == *surface_id
                            && surface.observed == SurfaceObservedState::Healthy
                    });
                if okf_ready {
                    knowledge.push(OkfCapabilityProjection::from_promoted(
                        &binding.receipt,
                        &binding.observation,
                    )?);
                }
            }
            knowledge.sort_by(|left, right| left.surface.cmp(&right.surface));
        }
    }
    if let Some(snapshot) = reconciliation.as_ref().filter(|_| active) {
        for surface in &extension.manifest.ui {
            if !snapshot.publishes(PluginSurfaceKind::Ui, &surface.id) {
                continue;
            }
            let dependencies = surface_graph
                .iter()
                .find(|candidate| {
                    candidate.surface.kind == PluginSurfaceKind::Ui
                        && candidate.surface.id == surface.id
                })
                .ok_or_else(|| {
                    UseError::new(
                        "use.capability.surface_graph_inconsistent",
                        format!(
                            "Published UI surface '{}' is missing from the canonical package surface graph.",
                            surface.id
                        ),
                    )
                })?
                .dependencies
                .clone();
            let mut styles = Vec::with_capacity(surface.styles.len());
            for path in &surface.styles {
                styles.push(activity_asset(receipt.package_root.join(path), "text/css").await?);
            }
            let mut scripts = Vec::with_capacity(surface.scripts.len());
            for path in &surface.scripts {
                scripts.push(
                    activity_asset(receipt.package_root.join(path), "text/javascript").await?,
                );
            }
            activity_bar.push(ActivityBarContribution {
                id: surface.id.clone(),
                title: surface.title.clone(),
                description: surface.description.clone(),
                icon: surface.icon.clone(),
                entry: activity_asset(receipt.package_root.join(&surface.entry), "text/html")
                    .await?,
                styles,
                scripts,
                skill: surface.skill.clone(),
                dependency_evidence_schema: UI_DEPENDENCY_EVIDENCE_SCHEMA.to_owned(),
                dependencies,
                order: surface.order,
            });
        }
    }
    let planner_evidence =
        plugin_planner_evidence(extension, context.desired_enabled, reconciliation.as_ref())?;
    // Keep Knowledge queryable while the rest of a multi-surface package is
    // still reconciling; other surfaces remain gated on `active` / publishes().
    let enabled = active || !knowledge.is_empty();
    Ok(CapabilityBinding {
        id: receipt.component_id.clone(),
        alias: receipt.route_alias.clone(),
        version: receipt.version.clone(),
        origin: CapabilityOrigin::Extension,
        enabled,
        readiness,
        reconciliation,
        planner_evidence,
        package_root: Some(receipt.package_root.clone()),
        lifecycle_generation: receipt.lifecycle_generation,
        requires_use: extension.manifest.requires_use.clone(),
        repository: extension
            .manifest
            .repository
            .as_ref()
            .map(|repository| RepositoryBinding {
                url: repository.url.clone(),
                revision: repository.revision.clone(),
            }),
        surfaces,
        mcp,
        mcp_servers,
        skills,
        flows,
        knowledge,
        activity_bar,
        tool_tasks,
        executable_tools,
    })
}

#[cfg(feature = "extensions")]
async fn surface_observations(
    extension: &a3s_use_extension::InstalledExtension,
    inspect_enabled_surfaces: bool,
    host_observations: &SurfaceObservations,
) -> UseResult<SurfaceObservations> {
    if host_observations.keys().any(|surface| match surface.kind {
        PluginSurfaceKind::Flow => !extension
            .manifest
            .flows
            .iter()
            .any(|flow| flow.id == surface.id),
        PluginSurfaceKind::Tool => !extension.manifest.tools.iter().any(|tool| {
            tool.id == surface.id
                && matches!(
                    &tool.workload,
                    a3s_use_extension::ToolWorkload::Task(task)
                        if !task.interactive
                            && matches!(
                                &task.source,
                                a3s_use_extension::ToolTaskSource::Release { .. }
                                    | a3s_use_extension::ToolTaskSource::Executable { .. }
                            )
                )
        }),
        PluginSurfaceKind::Okf => !extension
            .manifest
            .okf
            .iter()
            .any(|okf| okf.id == surface.id),
        PluginSurfaceKind::Mcp => !extension
            .manifest
            .mcp_servers
            .iter()
            .any(|mcp| mcp.id == surface.id),
        _ => true,
    }) {
        return Err(UseError::new(
            "use.capability.host_observation_invalid",
            "Production host observations must reference only their admitted Flow, Runtime/Executable Tool, MCP, or OKF surfaces.",
        ));
    }
    if !inspect_enabled_surfaces {
        return Ok(SurfaceObservations::new());
    }

    let mut observations = host_observations.clone();
    for surface in &extension.manifest.flows {
        if a3s_use_extension::inspect_flow_surface_file(surface, &extension.receipt.package_root)
            .await
            .is_err()
        {
            observations.insert(
                PluginSurfaceRef {
                    kind: PluginSurfaceKind::Flow,
                    id: surface.id.clone(),
                },
                SurfaceObservedState::Failed,
            );
        }
    }
    for surface in &extension.manifest.skills {
        let observed = match a3s_use_extension::inspect_skill_surface_file(
            surface,
            &extension.receipt.package_root,
        )
        .await
        {
            Ok(_) => SurfaceObservedState::Prepared,
            Err(_) => SurfaceObservedState::Failed,
        };
        observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Skill,
                id: surface.id.clone(),
            },
            observed,
        );
    }
    for surface in &extension.manifest.ui {
        let observed = match a3s_use_extension::inspect_ui_surface_files(
            surface,
            &extension.receipt.package_root,
        )
        .await
        {
            Ok(_) => SurfaceObservedState::Prepared,
            Err(_) => SurfaceObservedState::Failed,
        };
        observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: surface.id.clone(),
            },
            observed,
        );
    }
    Ok(observations)
}

#[cfg(feature = "extensions")]
async fn flow_observations_from_store(
    extension: &a3s_use_extension::InstalledExtension,
    store: &FlowRuntimeBindingStore,
    scope: &a3s_use_core::PlanScope,
) -> UseResult<SurfaceObservations> {
    let mut observations = SurfaceObservations::new();
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(observations);
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(observations);
    };
    let package_digest = format!("sha256:{package_sha256}");
    let manifest_digest = format!("sha256:{}", extension.receipt.manifest_sha256);
    for surface in &extension.manifest.flows {
        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: surface.id.clone(),
        };
        let qualified = PlanQualifiedSurfaceRef {
            package_id: extension.receipt.package_id.clone(),
            surface: reference.clone(),
        };
        let Some(binding) = store.get(scope, &qualified, generation).await? else {
            continue;
        };
        let state = if binding.package_digest() == package_digest
            && binding.manifest_digest() == manifest_digest
            && binding
                .inspect(surface, &extension.receipt.package_root)
                .await
                .is_ok()
        {
            SurfaceObservedState::Prepared
        } else {
            SurfaceObservedState::Failed
        };
        observations.insert(reference, state);
    }
    Ok(observations)
}

#[cfg(feature = "extensions")]
pub(crate) struct KnowledgeEvidence {
    pub(crate) bindings: Vec<OkfKnowledgeBinding>,
    pub(crate) failures: SurfaceObservations,
}

#[cfg(feature = "extensions")]
pub(crate) async fn knowledge_evidence_from_store(
    extension: &a3s_use_extension::InstalledExtension,
    store: &OkfKnowledgeBindingStore,
    client: &OkfKnowledgeClient,
    scope: &a3s_use_core::PlanScope,
) -> UseResult<KnowledgeEvidence> {
    let mut bindings = Vec::new();
    let mut failures = SurfaceObservations::new();
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(KnowledgeEvidence { bindings, failures });
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(KnowledgeEvidence { bindings, failures });
    };
    let package_digest = format!("sha256:{package_sha256}");
    let manifest_digest = format!("sha256:{}", extension.receipt.manifest_sha256);

    for surface in &extension.manifest.okf {
        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: surface.id.clone(),
        };
        let qualified = PlanQualifiedSurfaceRef {
            package_id: extension.receipt.package_id.clone(),
            surface: reference.clone(),
        };
        let Some(binding) = store.get(scope, &qualified, generation).await? else {
            continue;
        };
        let exact = binding.receipt.scope == *scope
            && binding.receipt.surface == qualified
            && binding.receipt.generation == generation
            && binding.receipt.package_digest == package_digest
            && binding.receipt.manifest_digest == manifest_digest
            && binding.receipt.bundle == surface.bundle;
        if !exact {
            failures.insert(reference, SurfaceObservedState::Failed);
            continue;
        }
        match client.observe(&binding.receipt).await {
            Ok(observed) if observed == binding => bindings.push(observed),
            Ok(_) | Err(_) => {
                failures.insert(reference, SurfaceObservedState::Failed);
            }
        }
    }
    Ok(KnowledgeEvidence { bindings, failures })
}

#[cfg(feature = "extensions")]
fn plugin_planner_evidence(
    extension: &a3s_use_extension::InstalledExtension,
    desired_enabled: bool,
    reconciliation: Option<&SurfaceReconcileSnapshot>,
) -> UseResult<Option<PluginPlannerEvidence>> {
    if extension.manifest.schema_version != 3 {
        return Ok(None);
    }
    let catalog = match extension.plan_ready_catalog() {
        Ok(catalog) => catalog,
        Err(error) if error.code == "use.extension.plan_evidence_missing" => return Ok(None),
        Err(error) => return Err(error),
    };
    let reconciliation = reconciliation.ok_or_else(|| {
        UseError::new(
            "use.capability.planner_evidence_invalid",
            "A plan-ready schema-v3 plugin omitted reconciliation evidence.",
        )
    })?;
    let mut selected_surfaces = reconciliation
        .surfaces
        .iter()
        .map(|surface| surface.surface.clone())
        .collect::<Vec<_>>();
    selected_surfaces.sort();
    selected_surfaces.dedup();
    let catalog_surfaces = catalog
        .record
        .surfaces
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    if selected_surfaces.is_empty() || selected_surfaces != catalog_surfaces {
        return Err(UseError::new(
            "use.capability.planner_evidence_invalid",
            "The installed manifest surface inventory does not match its verified catalog.",
        ));
    }
    let planned = catalog.selected_state(&selected_surfaces)?;
    let planned_surfaces = planned
        .release
        .surfaces
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    if planned_surfaces != selected_surfaces {
        return Err(UseError::new(
            "use.capability.planner_evidence_invalid",
            "The capability surface selection is not closed under catalog dependencies.",
        ));
    }
    let package_sha256 = extension.receipt.package_sha256.as_deref().ok_or_else(|| {
        UseError::new(
            "use.capability.planner_evidence_invalid",
            "A plan-ready receipt omitted its expanded-package digest.",
        )
    })?;
    Ok(Some(PluginPlannerEvidence {
        schema_version: PLANNER_EVIDENCE_SCHEMA_VERSION,
        package_id: extension.receipt.package_id.clone(),
        package_sha256: format!("sha256:{package_sha256}"),
        manifest_sha256: format!("sha256:{}", extension.receipt.manifest_sha256),
        receipt_digest: extension.receipt.descriptor_digest()?,
        catalog_record_digest: catalog.provenance.catalog_record_digest.clone(),
        desired_enabled,
        selected_surfaces,
    }))
}

#[cfg(feature = "extensions")]
pub(crate) fn installed_plugin_plan_evidence_from_snapshot(
    snapshot: &CapabilityRegistrySnapshot,
    extension: &a3s_use_extension::InstalledExtension,
) -> UseResult<InstalledPluginPlanEvidence> {
    let receipt = &extension.receipt;
    let binding = snapshot
        .capabilities
        .iter()
        .find(|binding| binding.id == receipt.component_id)
        .ok_or_else(|| {
            UseError::new(
                "use.capability.planner_evidence_missing",
                "The installed package is absent from the stable capability snapshot.",
            )
        })?;
    let summary = binding.planner_evidence.as_ref().ok_or_else(|| {
        UseError::new(
            "use.capability.planner_evidence_missing",
            "The installed package does not expose plan-ready capability evidence.",
        )
    })?;
    let catalog = extension.plan_ready_catalog()?.clone();
    let receipt_digest = receipt.descriptor_digest()?;
    let package_sha256 = catalog.record.package.sha256.as_deref();
    let manifest_sha256 = catalog.record.package.manifest_sha256.as_deref();
    if binding.origin != CapabilityOrigin::Extension
        || binding.version != receipt.version
        || summary.package_id != receipt.package_id
        || summary.package_sha256.as_str() != package_sha256.unwrap_or_default()
        || summary.manifest_sha256.as_str() != manifest_sha256.unwrap_or_default()
        || summary.receipt_digest != receipt_digest
        || summary.catalog_record_digest != catalog.provenance.catalog_record_digest
    {
        return Err(UseError::new(
            "use.capability.planner_evidence_invalid",
            "The package-specific receipt evidence does not match the stable capability snapshot.",
        ));
    }
    let evidence = InstalledPluginPlanEvidence {
        schema: INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA.to_owned(),
        component_id: receipt.component_id.clone(),
        package_id: receipt.package_id.clone(),
        version: receipt.version.clone(),
        capability_generation: snapshot.generation,
        capability_revision: snapshot.revision.clone(),
        receipt_digest,
        desired_enabled: summary.desired_enabled,
        selected_surfaces: summary.selected_surfaces.clone(),
        verified_catalog: catalog,
    };
    evidence.validate()?;
    Ok(evidence)
}
