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
    RuntimeProviderSelector, RuntimeSurfacePlan, RuntimeSurfacePlanPublication,
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

    /// Return the exact immutable Runtime plan payloads that must be
    /// published before this reviewed plan is committed. The publications
    /// carry no host paths and derive every lookup field from the selected
    /// plan/provider pair.
    pub fn runtime_plan_publications(&self) -> UseResult<Vec<RuntimeSurfacePlanPublication>> {
        self.providers.runtime_plan_publications()
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

    /// Build deterministic host-owned Runtime plan publications for all
    /// managed surfaces in this provider plan.
    pub fn runtime_plan_publications(&self) -> UseResult<Vec<RuntimeSurfacePlanPublication>> {
        self.runtime_selection.plan_publications()
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
#[path = "provider_plan_tests.rs"]
mod tests;
