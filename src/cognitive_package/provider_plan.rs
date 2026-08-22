use std::collections::{BTreeMap, BTreeSet};

use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{
    ExecutablePlanningSurface, PlanAuthority, PlanPackageChangeKind, PlanQualifiedSurfaceRef,
    PlanScope, PlannedPackageState, PlannedPackageTransition, PlannedProviderEvidence,
    PluginOperationAction, PluginOperationPlan, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginPackageLock, PluginPlanningBundle, PluginSurfaceKind,
    PluginWorkspaceGrantProposal, PluginWorkspaceGrantSnapshot, UseError, UseResult,
};

use crate::plugin_runtime::{
    plan_runtime_bundle, RuntimeProviderAssignment, RuntimeProviderSelection,
    RuntimeProviderSelector, RuntimeSurfacePlan,
};

use super::{
    bind_cognitive_package_grants, plan_native_provider_evidence, CognitivePackageGrantPlan,
};

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

/// Final host-bound plan plus the exact process-local Runtime selection that
/// produced its immutable provider evidence.
#[derive(Debug, Clone)]
pub struct BoundCognitivePackageProviderPlan {
    plan: PluginOperationPlan,
    grants: CognitivePackageGrantPlan,
    providers: CognitivePackageProviderPlan,
}

impl BoundCognitivePackageProviderPlan {
    pub fn plan(&self) -> &PluginOperationPlan {
        &self.plan
    }

    pub fn grants(&self) -> &CognitivePackageGrantPlan {
        &self.grants
    }

    pub fn providers(&self) -> &CognitivePackageProviderPlan {
        &self.providers
    }

    pub fn into_parts(
        self,
    ) -> (
        PluginOperationPlan,
        CognitivePackageGrantPlan,
        CognitivePackageProviderPlan,
    ) {
        (self.plan, self.grants, self.providers)
    }
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

    /// Require final provider observations to match an earlier capability
    /// preflight. Managed semantics may change only because the final
    /// canonical Grant proposal replaces the provisional proposal; provider
    /// identity, build, normalized capabilities, and enforcement may not.
    pub fn verify_preflight_evidence(
        &self,
        preflight: &[PlannedProviderEvidence],
    ) -> UseResult<()> {
        if preflight.len() != self.provider_evidence.len() {
            return Err(provider_evidence_changed());
        }
        for (expected, selected) in preflight.iter().zip(&self.provider_evidence) {
            let same_surface_and_provider = expected.surface == selected.surface
                && expected.provider_id == selected.provider_id
                && expected.provider_build_id == selected.provider_build_id
                && expected.capability_digest == selected.capability_digest
                && expected.enforcement == selected.enforcement;
            let native_semantics_match = expected.provider_id != "a3s-use-native-launcher"
                || expected.semantics_profile_digest == selected.semantics_profile_digest;
            if !same_surface_and_provider || !native_semantics_match {
                return Err(provider_evidence_changed());
            }
        }
        Ok(())
    }

    /// Require an apply-time reconstruction to equal the immutable reviewed
    /// evidence byte-for-byte, including the authorization-bound semantics
    /// profile.
    pub fn verify_reviewed_evidence(&self, reviewed: &[PlannedProviderEvidence]) -> UseResult<()> {
        if self.provider_evidence != reviewed {
            return Err(provider_evidence_changed());
        }
        Ok(())
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

/// Execute the authorization-safe two-pass provider protocol for one unbound
/// cognitive-package draft.
///
/// The first pass uses the provisional host binding only to query the exact
/// assigned providers and expose their enforcement to policy. The authority
/// callback then returns the host decision. The second pass rebuilds canonical
/// Grant proposals and Runtime semantics with that decision, reopens only the
/// same assignments, and rejects provider/build/capability/enforcement drift.
/// A final policy evaluation must return the same authority.
#[allow(clippy::too_many_arguments)]
pub async fn bind_cognitive_package_provider_plan<F>(
    mut draft: PluginOperationPlanDraft,
    provisional_binding: PluginOperationPlanBinding,
    grant_snapshot: &PluginWorkspaceGrantSnapshot,
    planning_bundles: &BTreeMap<String, PluginPlanningBundle>,
    generations: &BTreeMap<String, u64>,
    assignments: Vec<RuntimeProviderAssignment>,
    runtime_registry: &RuntimeClientRegistry,
    evaluate_authority: F,
) -> UseResult<BoundCognitivePackageProviderPlan>
where
    F: Fn(&PluginOperationPlan) -> UseResult<PlanAuthority>,
{
    draft.validate_unbound()?;

    let mut preflight_draft = draft.clone();
    let preflight_grants =
        bind_cognitive_package_grants(&mut preflight_draft, &provisional_binding, grant_snapshot)?;
    let preflight = plan_cognitive_package_providers(
        &preflight_draft.packages,
        planning_bundles,
        preflight_grants.proposals(),
        &provisional_binding.scope,
        generations,
        assignments.clone(),
        runtime_registry,
    )
    .await?;
    preflight_draft.providers = preflight.provider_evidence().to_vec();
    let preflight_plan = preflight_draft.bind(provisional_binding.clone())?;
    let authority = evaluate_authority(&preflight_plan)?;
    let final_binding = PluginOperationPlanBinding {
        authority: authority.clone(),
        ..provisional_binding
    };

    let grants = bind_cognitive_package_grants(&mut draft, &final_binding, grant_snapshot)?;
    let providers = plan_cognitive_package_providers(
        &draft.packages,
        planning_bundles,
        grants.proposals(),
        &final_binding.scope,
        generations,
        assignments,
        runtime_registry,
    )
    .await?;
    providers.verify_preflight_evidence(preflight.provider_evidence())?;
    draft.providers = providers.provider_evidence().to_vec();
    let plan = draft.bind(final_binding)?;
    if evaluate_authority(&plan)? != authority {
        return Err(provider_plan_error(
            "Final Grant-bound provider semantics changed the host authorization decision.",
        ));
    }

    Ok(BoundCognitivePackageProviderPlan {
        plan,
        grants,
        providers,
    })
}

/// Derive the exact lifecycle generations used by managed Runtime templates.
///
/// Added and replaced nodes use the same dependency-order/state-revision rule
/// as the cognitive-package saga. Retained and re-enabled nodes reuse their
/// immutable installed lifecycle generation. `installed_generations` may
/// contain unrelated installed packages, but every relevant retained or
/// replaced package must have one exact positive entry.
pub fn plan_cognitive_package_provider_generations(
    action: PluginOperationAction,
    packages: &[PlannedPackageTransition],
    state_revision: u64,
    package_lock: Option<&PluginPackageLock>,
    planning_bundles: &BTreeMap<String, PluginPlanningBundle>,
    installed_generations: &BTreeMap<String, u64>,
) -> UseResult<BTreeMap<String, u64>> {
    if state_revision == 0 {
        return Err(provider_plan_error(
            "Managed provider generation planning requires a positive state revision.",
        ));
    }
    if matches!(
        action,
        PluginOperationAction::Uninstall | PluginOperationAction::Disable
    ) {
        if !planning_bundles.is_empty() {
            return Err(provider_plan_error(
                "A retiring provider plan must not carry candidate planning bundles.",
            ));
        }
        return Ok(BTreeMap::new());
    }
    validate_package_order(packages)?;
    let states = selected_states(packages)?;
    validate_bundle_set(&states, planning_bundles)?;
    let managed_packages = states
        .keys()
        .filter(|package_id| {
            planning_bundles
                .get(*package_id)
                .is_some_and(has_managed_surfaces)
        })
        .map(String::as_str)
        .collect::<BTreeSet<_>>();
    let mut generations = BTreeMap::new();
    match action {
        PluginOperationAction::Install | PluginOperationAction::Upgrade => {
            let lock = package_lock.ok_or_else(|| {
                provider_plan_error(
                    "A managed graph provider plan omitted its candidate package lock.",
                )
            })?;
            lock.validate()?;
            let locked = lock
                .packages
                .iter()
                .map(|package| package.package_id())
                .collect::<BTreeSet<_>>();
            let selected = states.keys().map(String::as_str).collect::<BTreeSet<_>>();
            if locked != selected {
                return Err(provider_plan_error(
                    "The candidate package lock does not match the selected provider states.",
                ));
            }
            for (index, package) in lock.install_order()?.into_iter().enumerate() {
                if !managed_packages.contains(package.package_id()) {
                    continue;
                }
                let transition = packages
                    .iter()
                    .find(|transition| transition.package_id == package.package_id())
                    .ok_or_else(|| {
                        provider_plan_error(
                            "A managed package lock node omitted its reviewed transition.",
                        )
                    })?;
                let prior = installed_generations.get(package.package_id()).copied();
                let offset = u64::try_from(index).map_err(|_| {
                    provider_plan_error("The managed package generation offset is too large.")
                })?;
                let base = state_revision.checked_add(offset).ok_or_else(|| {
                    provider_plan_error("A managed package generation cannot advance.")
                })?;
                let generation = match transition.change {
                    PlanPackageChangeKind::Add => base,
                    PlanPackageChangeKind::Replace => base.max(
                        prior
                            .ok_or_else(|| {
                                provider_plan_error(
                                    "A replacement managed package omitted its prior generation.",
                                )
                            })?
                            .checked_add(1)
                            .ok_or_else(|| {
                                provider_plan_error(
                                    "A replacement managed package generation is exhausted.",
                                )
                            })?,
                    ),
                    PlanPackageChangeKind::Retain => prior.ok_or_else(|| {
                        provider_plan_error(
                            "A retained managed package omitted its installed generation.",
                        )
                    })?,
                    PlanPackageChangeKind::Remove => {
                        return Err(provider_plan_error(
                            "A removed package appeared in the candidate provider order.",
                        ))
                    }
                };
                if generation == 0 {
                    return Err(provider_plan_error(
                        "A managed package lifecycle generation must be positive.",
                    ));
                }
                generations.insert(package.package_id().to_owned(), generation);
            }
        }
        PluginOperationAction::Enable => {
            for package_id in &managed_packages {
                let generation =
                    installed_generations
                        .get(*package_id)
                        .copied()
                        .ok_or_else(|| {
                            provider_plan_error(
                        "An enabled managed package omitted its installed lifecycle generation.",
                    )
                        })?;
                if generation == 0 {
                    return Err(provider_plan_error(
                        "A managed package lifecycle generation must be positive.",
                    ));
                }
                generations.insert((*package_id).to_owned(), generation);
            }
        }
        PluginOperationAction::Uninstall | PluginOperationAction::Disable => {
            return Err(provider_plan_error(
                "A retiring provider action reached candidate generation planning.",
            ))
        }
    }
    if generations.len() != managed_packages.len() {
        return Err(provider_plan_error(
            "Managed lifecycle generations do not cover the exact managed package set.",
        ));
    }
    Ok(generations)
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

fn provider_evidence_changed() -> UseError {
    UseError::new(
        "use.plugin.runtime.provider_evidence_changed",
        "The selected Runtime provider evidence changed between reviewed lifecycle stages.",
    )
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
        PlanScopeKind, PlannedOperationImpact, PlannedPluginRelease, PlannedStateEvidence,
        PlanningArtifactRef, PlanningSurfaceActivation, PluginCatalogRecord, PluginPackageLockHost,
        PluginPackageResolver, PluginPermissionCeiling, PluginPlanSource, PluginReleaseChannel,
        PluginSurfaceRef, ResourcePermissionCeiling, SurfacePermissionCeiling, ToolWorkloadClass,
        VerifiedCatalogProvenance, VerifiedPluginCatalogRecord, WorkspaceGrantProposalAuthority,
        PLUGIN_PERMISSION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
        PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA, PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
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
        planned
            .verify_reviewed_evidence(planned.provider_evidence())
            .unwrap();

        let mut changed = planned.provider_evidence().to_vec();
        changed[1].provider_build_id = "build-2".to_owned();
        let error = planned.verify_preflight_evidence(&changed).unwrap_err();
        assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");

        let mut changed = planned.provider_evidence().to_vec();
        changed[1].semantics_profile_digest = DIGEST_D.to_owned();
        planned.verify_preflight_evidence(&changed).unwrap();
        let error = planned.verify_reviewed_evidence(&changed).unwrap_err();
        assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");
    }

    #[tokio::test]
    async fn two_pass_binding_replans_grant_semantics_without_provider_drift() {
        let (transition, bundle, _) = mixed_inputs();
        let package = transition.after.unwrap();
        let transition = PlannedPackageTransition::resolved(
            "acme/research",
            PlanPackageRole::Root,
            PlanPackageChangeKind::Add,
            None,
            Some(package),
            Some(PluginPlanSource::ReleaseBundle {
                bundle_digest: DIGEST_C.to_owned(),
                package_digest: DIGEST_A.to_owned(),
            }),
        )
        .unwrap();
        let draft = PluginOperationPlanDraft::new_unbound(
            PluginOperationAction::Install,
            "acme/research",
            "use/acme/research",
            vec![transition],
            Vec::new(),
            PlannedOperationImpact {
                download_bytes: 4096,
                installed_bytes_after: 8192,
                reclaimed_bytes: 0,
                drain_required: false,
                retained_data: false,
                okf_changes: Vec::new(),
            },
            PlannedStateEvidence {
                state_revision: 5,
                capability_generation: 4,
                receipt_digest: None,
            },
        )
        .unwrap();
        let provisional_binding = PluginOperationPlanBinding {
            operation_id: "install:provider-two-pass".to_owned(),
            created_at_ms: 10,
            expires_at_ms: 20,
            scope: scope(),
            authority: PlanAuthority {
                actor: PlanActor::User,
                decision: PlanPolicyDecision::Ask,
                policy_digest: DIGEST_C.to_owned(),
                confirmation_required: true,
            },
        };
        let snapshot = PluginWorkspaceGrantSnapshot {
            schema: PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA.to_owned(),
            scope_id: "workspace-01".to_owned(),
            state_revision: 5,
            grants: Vec::new(),
        };
        let mut registry = RuntimeClientRegistry::new();
        registry
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse("test-runtime").unwrap(),
                client: Arc::new(FakeRuntime::new(runtime_capabilities(), true)),
            }))
            .unwrap();
        let assignment = RuntimeProviderAssignment::new(
            PlanQualifiedSurfaceRef {
                package_id: "acme/research".to_owned(),
                surface: PluginSurfaceRef {
                    kind: PluginSurfaceKind::Tool,
                    id: "index".to_owned(),
                },
            },
            "test-runtime",
        )
        .unwrap();

        let bound = bind_cognitive_package_provider_plan(
            draft,
            provisional_binding,
            &snapshot,
            &BTreeMap::from([("acme/research".to_owned(), bundle)]),
            &BTreeMap::from([("acme/research".to_owned(), 8)]),
            vec![assignment],
            &registry,
            |_| {
                Ok(PlanAuthority {
                    actor: PlanActor::User,
                    decision: PlanPolicyDecision::Allow,
                    policy_digest: DIGEST_D.to_owned(),
                    confirmation_required: false,
                })
            },
        )
        .await
        .unwrap();

        assert_eq!(bound.plan().providers.len(), 2);
        assert_eq!(bound.plan().authority.decision, PlanPolicyDecision::Allow);
        assert_eq!(bound.providers().runtime_selection().surfaces().len(), 1);
        assert_eq!(
            bound
                .grants()
                .proposal("acme/research")
                .unwrap()
                .authority
                .decision,
            PlanPolicyDecision::Allow
        );
    }

    #[test]
    fn managed_provider_generations_follow_add_replace_and_retain_lifecycles() {
        let lock = provider_package_lock();
        let (_, bundle, _) = mixed_inputs();
        let bundles = BTreeMap::from([("acme/research".to_owned(), bundle)]);

        let add = provider_transition(PlanPackageChangeKind::Add);
        let generations = plan_cognitive_package_provider_generations(
            PluginOperationAction::Install,
            &[add],
            7,
            Some(&lock),
            &bundles,
            &BTreeMap::new(),
        )
        .unwrap();
        assert_eq!(
            generations,
            BTreeMap::from([("acme/research".to_owned(), 7)])
        );

        let replace = provider_transition(PlanPackageChangeKind::Replace);
        let generations = plan_cognitive_package_provider_generations(
            PluginOperationAction::Upgrade,
            &[replace],
            7,
            Some(&lock),
            &bundles,
            &BTreeMap::from([("acme/research".to_owned(), 11)]),
        )
        .unwrap();
        assert_eq!(
            generations,
            BTreeMap::from([("acme/research".to_owned(), 12)])
        );

        let retain = provider_transition(PlanPackageChangeKind::Retain);
        let generations = plan_cognitive_package_provider_generations(
            PluginOperationAction::Upgrade,
            &[retain],
            7,
            Some(&lock),
            &bundles,
            &BTreeMap::from([("acme/research".to_owned(), 11)]),
        )
        .unwrap();
        assert_eq!(
            generations,
            BTreeMap::from([("acme/research".to_owned(), 11)])
        );
    }

    #[test]
    fn managed_provider_generations_reject_missing_or_exhausted_prior_generation() {
        let lock = provider_package_lock();
        let (_, bundle, _) = mixed_inputs();
        let bundles = BTreeMap::from([("acme/research".to_owned(), bundle)]);
        let replace = provider_transition(PlanPackageChangeKind::Replace);

        let missing = plan_cognitive_package_provider_generations(
            PluginOperationAction::Upgrade,
            std::slice::from_ref(&replace),
            7,
            Some(&lock),
            &bundles,
            &BTreeMap::new(),
        )
        .unwrap_err();
        assert_eq!(missing.code, "use.plugin.provider_plan_invalid");
        assert!(missing.message.contains("omitted its prior generation"));

        let exhausted = plan_cognitive_package_provider_generations(
            PluginOperationAction::Upgrade,
            &[replace],
            7,
            Some(&lock),
            &bundles,
            &BTreeMap::from([("acme/research".to_owned(), u64::MAX)]),
        )
        .unwrap_err();
        assert_eq!(exhausted.code, "use.plugin.provider_plan_invalid");
        assert!(exhausted.message.contains("generation is exhausted"));
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

    fn provider_transition(change: PlanPackageChangeKind) -> PlannedPackageTransition {
        let (transition, _, _) = mixed_inputs();
        let after = transition.after.unwrap();
        match change {
            PlanPackageChangeKind::Add => PlannedPackageTransition::resolved(
                "acme/research",
                PlanPackageRole::Root,
                change,
                None,
                Some(after),
                Some(PluginPlanSource::ReleaseBundle {
                    bundle_digest: DIGEST_C.to_owned(),
                    package_digest: DIGEST_A.to_owned(),
                }),
            ),
            PlanPackageChangeKind::Replace => {
                let mut before = after.clone();
                before.release.version = "1.0.0".to_owned();
                before.release.package_sha256 = DIGEST_D.to_owned();
                before.release.manifest_sha256 = DIGEST_C.to_owned();
                PlannedPackageTransition::resolved(
                    "acme/research",
                    PlanPackageRole::Root,
                    change,
                    Some(before),
                    Some(after),
                    Some(PluginPlanSource::ReleaseBundle {
                        bundle_digest: DIGEST_C.to_owned(),
                        package_digest: DIGEST_A.to_owned(),
                    }),
                )
            }
            PlanPackageChangeKind::Retain => PlannedPackageTransition::resolved(
                "acme/research",
                PlanPackageRole::Root,
                change,
                Some(after.clone()),
                Some(after),
                None,
            ),
            PlanPackageChangeKind::Remove => unreachable!(),
        }
        .unwrap()
    }

    fn provider_package_lock() -> PluginPackageLock {
        let record = PluginCatalogRecord::from_json(include_bytes!(
            "../../crates/core/fixtures/plugins/catalog-record-v3.json"
        ))
        .unwrap();
        let provenance = VerifiedCatalogProvenance {
            registry_name: "official".to_owned(),
            registry_url: "https://packages.example.test/catalog/".to_owned(),
            root_sha256: DIGEST_D.to_owned(),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest: record.descriptor_digest().unwrap(),
        };
        let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
        PluginPackageResolver::new(
            PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
        )
        .resolve(verified, Vec::new())
        .unwrap()
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
