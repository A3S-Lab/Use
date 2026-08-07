use std::collections::{BTreeMap, BTreeSet};

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{
    ExecutablePlanningSurface, PlanQualifiedSurfaceRef, PlanScope, PlannedPackageState,
    PlannedPackageTransition, PlannedProviderEvidence, PluginPlanningBundle, PluginSurfaceKind,
    PluginWorkspaceGrantProposal, UseError, UseResult,
};

use crate::plugin_runtime::{
    plan_runtime_bundle, RuntimeProviderAssignment, RuntimeProviderSelection,
    RuntimeProviderSelector, RuntimeSurfacePlan,
};

use super::plan_native_provider_evidence;

/// Complete provider result for one reviewed cognitive-package transition set.
///
/// `provider_evidence` covers every selected Tool and MCP surface, including
/// package-native launchers. `runtime_selection` contains only release-backed
/// Runtime surfaces and retains their exact process-local clients for apply.
#[derive(Debug, Clone)]
pub struct CognitivePackageProviderPlan {
    provider_evidence: Vec<PlannedProviderEvidence>,
    runtime_selection: RuntimeProviderSelection,
}

impl CognitivePackageProviderPlan {
    pub fn provider_evidence(&self) -> &[PlannedProviderEvidence] {
        &self.provider_evidence
    }

    pub fn runtime_selection(&self) -> &RuntimeProviderSelection {
        &self.runtime_selection
    }

    pub fn into_parts(self) -> (Vec<PlannedProviderEvidence>, RuntimeProviderSelection) {
        (self.provider_evidence, self.runtime_selection)
    }
}

/// Plan native and managed providers as one exact, fail-closed package set.
///
/// The host supplies canonical pre-confirmation Grant proposals, one positive
/// lifecycle generation per package containing managed surfaces, one explicit
/// assignment per managed surface, and its configured Runtime registry. The
/// function never chooses a default provider and never falls back to native
/// execution when a selected Runtime is absent or incapable.
pub async fn plan_cognitive_package_providers(
    packages: &[PlannedPackageTransition],
    planning_bundles: &BTreeMap<String, PluginPlanningBundle>,
    grant_proposals: &BTreeMap<String, PluginWorkspaceGrantProposal>,
    scope: &PlanScope,
    generations: &BTreeMap<String, u64>,
    assignments: Vec<RuntimeProviderAssignment>,
    runtime_registry: &RuntimeClientRegistry,
) -> UseResult<CognitivePackageProviderPlan> {
    validate_package_order(packages)?;
    let states = selected_states(packages)?;
    validate_bundle_set(&states, planning_bundles)?;
    validate_grant_proposals(&states, grant_proposals, scope)?;

    let managed_packages = states
        .keys()
        .filter_map(|package_id| {
            planning_bundles
                .get(package_id)
                .is_some_and(has_managed_surfaces)
                .then_some(package_id.as_str())
        })
        .collect::<BTreeSet<_>>();
    validate_generations(&managed_packages, generations)?;

    let mut runtime_plans = Vec::<RuntimeSurfacePlan>::new();
    for package_id in &managed_packages {
        let package = states.get(*package_id).ok_or_else(|| {
            provider_plan_error("A managed package lost its selected package state.")
        })?;
        let bundle = planning_bundles
            .get(*package_id)
            .ok_or_else(|| provider_plan_error("A managed package lost its planning bundle."))?;
        let proposal = grant_proposals.get(*package_id).ok_or_else(|| {
            provider_plan_error(format!(
                "Managed package '{package_id}' omitted its canonical Grant proposal."
            ))
        })?;
        let generation = generations.get(*package_id).copied().ok_or_else(|| {
            provider_plan_error(format!(
                "Managed package '{package_id}' omitted its exact lifecycle generation."
            ))
        })?;
        runtime_plans.extend(plan_runtime_bundle(
            bundle, package, proposal, scope, generation,
        )?);
    }

    let runtime_selection = RuntimeProviderSelector::new(runtime_registry)
        .select(runtime_plans, assignments)
        .await?;
    let mut provider_evidence = plan_native_provider_evidence(packages, planning_bundles)?;
    provider_evidence.extend(runtime_selection.provider_evidence());
    provider_evidence.sort_by(|left, right| left.surface.cmp(&right.surface));
    validate_complete_evidence(&states, &provider_evidence)?;

    Ok(CognitivePackageProviderPlan {
        provider_evidence,
        runtime_selection,
    })
}

fn validate_package_order(packages: &[PlannedPackageTransition]) -> UseResult<()> {
    if packages
        .windows(2)
        .any(|pair| pair[0].package_id >= pair[1].package_id)
    {
        return Err(provider_plan_error(
            "Cognitive-package provider inputs must be sorted uniquely by package ID.",
        ));
    }
    Ok(())
}

fn selected_states(
    packages: &[PlannedPackageTransition],
) -> UseResult<BTreeMap<String, &PlannedPackageState>> {
    let mut states = BTreeMap::new();
    for package in packages {
        let Some(state) = package.after.as_ref() else {
            continue;
        };
        if state.release.package_id != package.package_id
            || states.insert(package.package_id.clone(), state).is_some()
        {
            return Err(provider_plan_error(
                "A selected package state does not match its unique transition identity.",
            ));
        }
    }
    Ok(states)
}

fn validate_bundle_set(
    states: &BTreeMap<String, &PlannedPackageState>,
    planning_bundles: &BTreeMap<String, PluginPlanningBundle>,
) -> UseResult<()> {
    let expected = states
        .iter()
        .filter(|(_, state)| has_executable_surfaces(state))
        .map(|(package_id, _)| package_id.as_str())
        .collect::<BTreeSet<_>>();
    let actual = planning_bundles
        .keys()
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    if actual != expected {
        return Err(provider_plan_error(
            "Planning bundles must cover exactly the selected executable packages.",
        ));
    }
    Ok(())
}

fn validate_grant_proposals(
    states: &BTreeMap<String, &PlannedPackageState>,
    proposals: &BTreeMap<String, PluginWorkspaceGrantProposal>,
    scope: &PlanScope,
) -> UseResult<()> {
    for (package_id, proposal) in proposals {
        let state = states.get(package_id).ok_or_else(|| {
            provider_plan_error(
                "A canonical Grant proposal names a package outside the selected transition set.",
            )
        })?;
        proposal.validate_against(&state.permissions)?;
        if proposal.scope_id != scope.id
            || proposal.package_id != *package_id
            || proposal.package_digest != state.release.package_sha256
            || proposal.permission_ceiling_digest != state.release.permission_ceiling_digest
            || proposal.permissions != state.permissions
        {
            return Err(provider_plan_error(
                "A canonical Grant proposal does not bind the selected package state and scope.",
            ));
        }
    }
    Ok(())
}

fn validate_generations(
    managed_packages: &BTreeSet<&str>,
    generations: &BTreeMap<String, u64>,
) -> UseResult<()> {
    let actual = generations
        .iter()
        .map(|(package_id, generation)| (package_id.as_str(), *generation))
        .collect::<BTreeMap<_, _>>();
    let actual_packages = actual.keys().copied().collect::<BTreeSet<_>>();
    if &actual_packages != managed_packages || actual.values().any(|generation| *generation == 0) {
        return Err(provider_plan_error(
            "Managed Runtime generations must cover exactly the managed package set.",
        ));
    }
    Ok(())
}

fn validate_complete_evidence(
    states: &BTreeMap<String, &PlannedPackageState>,
    providers: &[PlannedProviderEvidence],
) -> UseResult<()> {
    let expected = states
        .iter()
        .flat_map(|(package_id, state)| {
            state
                .release
                .surfaces
                .iter()
                .filter(|surface| {
                    matches!(
                        surface.kind,
                        PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                    )
                })
                .map(|surface| PlanQualifiedSurfaceRef {
                    package_id: package_id.clone(),
                    surface: surface.reference(),
                })
        })
        .collect::<Vec<_>>();
    if providers.len() != expected.len()
        || providers
            .iter()
            .zip(expected)
            .any(|(provider, expected)| provider.surface != expected)
    {
        return Err(provider_plan_error(
            "The combined provider evidence does not cover the exact executable surface set.",
        ));
    }
    Ok(())
}

fn has_executable_surfaces(state: &PlannedPackageState) -> bool {
    state.release.surfaces.iter().any(|surface| {
        matches!(
            surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )
    })
}

fn has_managed_surfaces(bundle: &PluginPlanningBundle) -> bool {
    bundle.surfaces.iter().any(|surface| {
        !matches!(
            surface,
            ExecutablePlanningSurface::ToolTaskNative { .. }
                | ExecutablePlanningSurface::McpStdio { .. }
        )
    })
}

fn provider_plan_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.provider_plan_invalid", message)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use a3s_runtime::contract::{
        HealthCheckKind, IsolationLevel, MountKind, NetworkMode, ResourceControl,
        RuntimeCapabilities, RuntimeFeature, RuntimeUnitClass,
    };
    use a3s_runtime::{ProviderId, RuntimeClient, RuntimeProviderFactory, RuntimeResult};
    use a3s_use_core::{
        CatalogSurface, PlanActor, PlanPackageChangeKind, PlanPackageRole, PlanPolicyDecision,
        PlanScopeKind, PlannedPluginRelease, PlanningArtifactRef, PlanningSurfaceActivation,
        PluginPermissionCeiling, PluginReleaseChannel, PluginSurfaceRef, ResourcePermissionCeiling,
        SurfacePermissionCeiling, ToolWorkloadClass, WorkspaceGrantProposalAuthority,
        PLUGIN_PERMISSION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
        PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA,
    };
    use async_trait::async_trait;

    use crate::plugin_runtime::test_support::{service_descriptor, FakeRuntime, DIGEST_A};

    use super::*;

    const DIGEST_B: &str =
        "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const DIGEST_C: &str =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const DIGEST_D: &str =
        "sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";

    struct StaticRuntimeFactory {
        provider_id: ProviderId,
        client: Arc<dyn RuntimeClient>,
    }

    #[async_trait]
    impl RuntimeProviderFactory for StaticRuntimeFactory {
        fn provider_id(&self) -> &ProviderId {
            &self.provider_id
        }

        async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
            Ok(self.client.clone())
        }
    }

    #[tokio::test]
    async fn mixed_package_produces_complete_native_and_managed_provider_plan() {
        let (transition, bundle, proposal) = mixed_inputs();
        let capabilities = runtime_capabilities();
        let mut registry = RuntimeClientRegistry::new();
        registry
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse("test-runtime").unwrap(),
                client: Arc::new(FakeRuntime::new(capabilities, true)),
            }))
            .unwrap();
        let managed_surface = PlanQualifiedSurfaceRef {
            package_id: "acme/research".to_owned(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "index".to_owned(),
            },
        };

        let planned = plan_cognitive_package_providers(
            &[transition],
            &BTreeMap::from([("acme/research".to_owned(), bundle)]),
            &BTreeMap::from([("acme/research".to_owned(), proposal)]),
            &scope(),
            &BTreeMap::from([("acme/research".to_owned(), 8)]),
            vec![RuntimeProviderAssignment::new(managed_surface, "test-runtime").unwrap()],
            &registry,
        )
        .await
        .unwrap();

        assert_eq!(planned.provider_evidence().len(), 2);
        assert_eq!(
            planned.provider_evidence()[0].provider_id,
            "a3s-use-native-launcher"
        );
        assert_eq!(planned.provider_evidence()[1].provider_id, "test-runtime");
        assert_eq!(planned.runtime_selection().surfaces().len(), 1);
        assert_eq!(
            planned.runtime_selection().surfaces()[0]
                .plan()
                .spec()
                .generation,
            8
        );
    }

    #[tokio::test]
    async fn managed_provider_plan_rejects_missing_or_extra_host_evidence() {
        let (transition, bundle, proposal) = mixed_inputs();
        let bundles = BTreeMap::from([("acme/research".to_owned(), bundle)]);
        let proposals = BTreeMap::from([("acme/research".to_owned(), proposal)]);
        let registry = RuntimeClientRegistry::new();

        let missing_proposal = plan_cognitive_package_providers(
            std::slice::from_ref(&transition),
            &bundles,
            &BTreeMap::new(),
            &scope(),
            &BTreeMap::from([("acme/research".to_owned(), 8)]),
            Vec::new(),
            &registry,
        )
        .await
        .unwrap_err();
        assert_eq!(missing_proposal.code, "use.plugin.provider_plan_invalid");

        let extra_generation = plan_cognitive_package_providers(
            &[transition],
            &bundles,
            &proposals,
            &scope(),
            &BTreeMap::from([
                ("acme/research".to_owned(), 8),
                ("acme/unrelated".to_owned(), 9),
            ]),
            Vec::new(),
            &registry,
        )
        .await
        .unwrap_err();
        assert_eq!(extra_generation.code, "use.plugin.provider_plan_invalid");
    }

    #[tokio::test]
    async fn selected_runtime_disappearance_never_falls_back_to_native() {
        let (transition, bundle, proposal) = mixed_inputs();
        let managed_surface = PlanQualifiedSurfaceRef {
            package_id: "acme/research".to_owned(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "index".to_owned(),
            },
        };
        let error = plan_cognitive_package_providers(
            &[transition],
            &BTreeMap::from([("acme/research".to_owned(), bundle)]),
            &BTreeMap::from([("acme/research".to_owned(), proposal)]),
            &scope(),
            &BTreeMap::from([("acme/research".to_owned(), 8)]),
            vec![RuntimeProviderAssignment::new(managed_surface, "missing-runtime").unwrap()],
            &RuntimeClientRegistry::new(),
        )
        .await
        .unwrap_err();

        assert_ne!(error.code, "use.plugin.provider_plan_invalid");
        assert!(error.message.contains("selected Runtime provider"));
    }

    fn mixed_inputs() -> (
        PlannedPackageTransition,
        PluginPlanningBundle,
        PluginWorkspaceGrantProposal,
    ) {
        let descriptor = service_descriptor();
        let native_surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        };
        let managed_surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "index".to_owned(),
        };
        let permissions = PluginPermissionCeiling {
            schema: PLUGIN_PERMISSION_SCHEMA.to_owned(),
            surfaces: vec![
                SurfacePermissionCeiling {
                    surface: native_surface.clone(),
                    native_execution: true,
                    child_process: false,
                    filesystem: Vec::new(),
                    network_egress: Vec::new(),
                    private_service: false,
                    secrets: Vec::new(),
                    resources: Some(ResourcePermissionCeiling {
                        cpu_millis: 500,
                        memory_bytes: 256 * 1024 * 1024,
                        pids: 64,
                        ephemeral_storage_bytes: 512 * 1024 * 1024,
                        task_timeout_ms: Some(120_000),
                        max_stdout_bytes: Some(4 * 1024 * 1024),
                        max_stderr_bytes: Some(1024 * 1024),
                    }),
                    ui_http: Vec::new(),
                },
                SurfacePermissionCeiling {
                    surface: managed_surface.clone(),
                    native_execution: false,
                    child_process: false,
                    filesystem: Vec::new(),
                    network_egress: Vec::new(),
                    private_service: true,
                    secrets: Vec::new(),
                    resources: Some(ResourcePermissionCeiling {
                        cpu_millis: 500,
                        memory_bytes: 256 * 1024 * 1024,
                        pids: 64,
                        ephemeral_storage_bytes: 512 * 1024 * 1024,
                        task_timeout_ms: None,
                        max_stdout_bytes: None,
                        max_stderr_bytes: None,
                    }),
                    ui_http: Vec::new(),
                },
            ],
        };
        let permission_digest = permissions.descriptor_digest().unwrap();
        let package = PlannedPackageState {
            release: PlannedPluginRelease {
                package_id: "acme/research".to_owned(),
                version: "2.0.0".to_owned(),
                channel: PluginReleaseChannel::Stable,
                target: "linux-x86_64".to_owned(),
                package_sha256: DIGEST_A.to_owned(),
                manifest_sha256: DIGEST_B.to_owned(),
                permission_ceiling_digest: permission_digest.clone(),
                surfaces: vec![
                    CatalogSurface {
                        kind: PluginSurfaceKind::Tool,
                        id: native_surface.id.clone(),
                        optional: false,
                        workload: Some(ToolWorkloadClass::Task),
                        mcp_transport: None,
                        mcp_tool_count: None,
                        okf_bundle: None,
                        requires: Vec::new(),
                    },
                    CatalogSurface {
                        kind: PluginSurfaceKind::Tool,
                        id: managed_surface.id.clone(),
                        optional: false,
                        workload: Some(ToolWorkloadClass::Service),
                        mcp_transport: None,
                        mcp_tool_count: None,
                        okf_bundle: None,
                        requires: Vec::new(),
                    },
                ],
            },
            permissions: permissions.clone(),
        };
        let bundle = PluginPlanningBundle {
            schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
            package_id: package.release.package_id.clone(),
            version: package.release.version.clone(),
            channel: package.release.channel,
            target: package.release.target.clone(),
            archive_sha256: DIGEST_C.to_owned(),
            package_sha256: package.release.package_sha256.clone(),
            manifest_sha256: package.release.manifest_sha256.clone(),
            permission_ceiling_digest: permission_digest.clone(),
            surfaces: vec![
                ExecutablePlanningSurface::ToolTaskNative {
                    id: native_surface.id,
                    activation: PlanningSurfaceActivation::Lazy,
                    executable: "bin/acme-research".to_owned(),
                    command: "acme-convert".to_owned(),
                    json_output: true,
                    timeout_ms: 120_000,
                },
                ExecutablePlanningSurface::ToolService {
                    id: managed_surface.id,
                    activation: PlanningSurfaceActivation::Eager,
                    base_path: "/api".to_owned(),
                    artifact: PlanningArtifactRef {
                        uri: format!(
                            "oci://registry.example/acme/research-index@{}",
                            descriptor.artifact.digest
                        ),
                        digest: descriptor.artifact.digest.clone(),
                        media_type: descriptor.artifact.media_type.clone(),
                    },
                    descriptor,
                },
            ],
        };
        let proposal = PluginWorkspaceGrantProposal {
            schema: PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA.to_owned(),
            operation_id: "install:provider-plan".to_owned(),
            scope_id: "workspace-01".to_owned(),
            package_id: package.release.package_id.clone(),
            package_digest: package.release.package_sha256.clone(),
            permission_ceiling_digest: permission_digest.clone(),
            permissions_digest: permission_digest,
            permissions,
            authority: WorkspaceGrantProposalAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Ask,
                policy_digest: DIGEST_D.to_owned(),
            },
            created_at_ms: 1,
            apply_expires_at_ms: 2,
            grant_expires_at_ms: None,
        };
        (
            PlannedPackageTransition {
                package_id: "acme/research".to_owned(),
                role: PlanPackageRole::Root,
                change: PlanPackageChangeKind::Add,
                before: None,
                after: Some(package),
                source: None,
                surfaces: Vec::new(),
            },
            bundle,
            proposal,
        )
    }

    fn scope() -> PlanScope {
        PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "workspace-01".to_owned(),
        }
    }

    fn runtime_capabilities() -> RuntimeCapabilities {
        RuntimeCapabilities {
            schema: RuntimeCapabilities::SCHEMA.to_owned(),
            provider_id: ProviderId::parse("test-runtime").unwrap(),
            provider_build: "build-1".to_owned(),
            unit_classes: vec![RuntimeUnitClass::Service],
            artifact_media_types: vec!["application/vnd.oci.image.index.v1+json".to_owned()],
            isolation_levels: vec![IsolationLevel::Container],
            network_modes: vec![NetworkMode::Service],
            mount_kinds: Vec::<MountKind>::new(),
            health_check_kinds: vec![HealthCheckKind::Http],
            resource_controls: vec![
                ResourceControl::Cpu,
                ResourceControl::Memory,
                ResourceControl::Pids,
                ResourceControl::EphemeralStorage,
            ],
            features: vec![
                RuntimeFeature::DurableIdentity,
                RuntimeFeature::ServiceTcp,
                RuntimeFeature::Logs,
                RuntimeFeature::Stop,
                RuntimeFeature::Remove,
            ],
        }
    }
}
