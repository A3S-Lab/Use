use a3s_runtime::RuntimeClientRegistry;
use a3s_use_core::{
    PlannedPackageState, PlannedProviderEvidence, PluginPlanningBundle, PluginWorkspaceGrantPlan,
    PluginWorkspaceGrantProposal, UseError, UseResult,
};

use super::bundle_planner::plan_runtime_bundle_with_authorization;
use super::{
    plan_runtime_bundle_with_authority, RuntimeAuthorityBindings, RuntimeAuthorityResolverRegistry,
    RuntimeProviderAssignment, RuntimeProviderSelection, RuntimeProviderSelector,
    SelectedRuntimeSurface,
};

const PREFLIGHT_AUTHORIZATION_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";

/// Host composition boundary for two-pass executable plugin planning.
///
/// The broker connects only explicitly assigned Runtime providers. Preflight
/// proves capability and enforcement before host authorization. Authorization
/// then binds the canonical grant proposal into each Runtime semantics profile
/// while retaining and rechecking the exact preflight clients.
pub struct PluginRuntimeBroker<'a> {
    registry: &'a RuntimeClientRegistry,
}

impl<'a> PluginRuntimeBroker<'a> {
    pub fn new(registry: &'a RuntimeClientRegistry) -> Self {
        Self { registry }
    }

    pub async fn preflight_bundle(
        &self,
        bundle: PluginPlanningBundle,
        package: PlannedPackageState,
        scope_id: impl Into<String>,
        generation: u64,
        assignments: Vec<RuntimeProviderAssignment>,
    ) -> UseResult<RuntimeBundlePreflight> {
        self.preflight_bundle_with_authority(
            bundle,
            package,
            scope_id,
            generation,
            RuntimeAuthorityBindings::default(),
            assignments,
        )
        .await
    }

    /// Resolve provider-bound host authority, then preflight the exact same
    /// assignments and resources against the selected Runtime clients.
    pub async fn preflight_bundle_with_resolvers(
        &self,
        bundle: PluginPlanningBundle,
        package: PlannedPackageState,
        scope_id: impl Into<String>,
        generation: u64,
        resolvers: &RuntimeAuthorityResolverRegistry,
        assignments: Vec<RuntimeProviderAssignment>,
    ) -> UseResult<RuntimeBundlePreflight> {
        let scope_id = scope_id.into();
        let authority = resolvers
            .resolve_bindings(&bundle, &package, &scope_id, generation, &assignments)
            .await?;
        self.preflight_bundle_with_authority(
            bundle,
            package,
            scope_id,
            generation,
            authority,
            assignments,
        )
        .await
    }

    /// Preflight a bundle with exact host-owned filesystem and secret
    /// references. The bindings are retained unchanged for final
    /// grant-proposal authorization.
    pub async fn preflight_bundle_with_authority(
        &self,
        bundle: PluginPlanningBundle,
        package: PlannedPackageState,
        scope_id: impl Into<String>,
        generation: u64,
        authority: RuntimeAuthorityBindings,
        assignments: Vec<RuntimeProviderAssignment>,
    ) -> UseResult<RuntimeBundlePreflight> {
        let scope_id = scope_id.into();
        let plans = plan_runtime_bundle_with_authorization(
            &bundle,
            &package,
            &scope_id,
            PREFLIGHT_AUTHORIZATION_DIGEST,
            &authority,
            &assignments,
            generation,
        )?;
        let selection = RuntimeProviderSelector::new(self.registry)
            .select(plans, assignments.clone())
            .await?;
        Ok(RuntimeBundlePreflight {
            bundle,
            package,
            scope_id,
            generation,
            authority,
            assignments,
            selection,
        })
    }
}

/// Process-local preflight state retained across host policy evaluation.
///
/// Provider clients are intentionally not serializable. The provisional
/// evidence may be used only to evaluate host policy; `authorize` replaces its
/// placeholder semantics digest with the exact grant-proposal-bound digest.
pub struct RuntimeBundlePreflight {
    bundle: PluginPlanningBundle,
    package: PlannedPackageState,
    scope_id: String,
    generation: u64,
    authority: RuntimeAuthorityBindings,
    assignments: Vec<RuntimeProviderAssignment>,
    selection: RuntimeProviderSelection,
}

impl std::fmt::Debug for RuntimeBundlePreflight {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RuntimeBundlePreflight")
            .field("package_id", &self.bundle.package_id)
            .field("scope_id", &self.scope_id)
            .field("generation", &self.generation)
            .field("authority_surfaces", &self.authority.surfaces().len())
            .field("provider_evidence", &self.selection.provider_evidence())
            .finish()
    }
}

impl RuntimeBundlePreflight {
    pub fn bundle(&self) -> &PluginPlanningBundle {
        &self.bundle
    }

    pub fn package(&self) -> &PlannedPackageState {
        &self.package
    }

    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn provisional_provider_evidence(&self) -> Vec<PlannedProviderEvidence> {
        self.selection.provider_evidence()
    }

    pub async fn authorize(
        self,
        proposal: &PluginWorkspaceGrantProposal,
    ) -> UseResult<RuntimeProviderSelection> {
        if proposal.scope_id != self.scope_id {
            return Err(preflight_mismatch(
                "The grant proposal scope does not match Runtime preflight.",
            ));
        }
        let plans = plan_runtime_bundle_with_authority(
            &self.bundle,
            &self.package,
            proposal,
            &self.authority,
            &self.assignments,
            self.generation,
        )?;
        let selected = self.selection.surfaces();
        if plans.len() != selected.len()
            || plans
                .iter()
                .zip(selected)
                .any(|(plan, preflight)| plan.surface() != preflight.plan().surface())
        {
            return Err(preflight_mismatch(
                "Authorized Runtime surfaces do not match preflight.",
            ));
        }

        let mut surfaces = Vec::with_capacity(plans.len());
        for (plan, preflight) in plans.into_iter().zip(selected) {
            let semantics_profile_digest = plan
                .spec()
                .semantics_profile_digest
                .clone()
                .ok_or_else(|| {
                    preflight_mismatch(
                        "An authorized Runtime surface omitted its semantics digest.",
                    )
                })?;
            let provisional = preflight.provider();
            let provider = PlannedProviderEvidence {
                surface: plan.surface(),
                provider_id: provisional.provider_id.clone(),
                provider_build_id: provisional.provider_build_id.clone(),
                capability_digest: provisional.capability_digest.clone(),
                semantics_profile_digest,
                enforcement: provisional.enforcement,
            };
            preflight.client().verify_plan(&plan, &provider).await?;
            surfaces.push(SelectedRuntimeSurface::from_parts(
                plan,
                provider,
                preflight.client().clone(),
            ));
        }
        Ok(RuntimeProviderSelection::from_surfaces(surfaces))
    }

    /// Finalize provider selection from the canonical host grant plan.
    ///
    /// Multi-package grant plans are allowed, but this preflight consumes only
    /// the exact candidate proposal for its own immutable package identity.
    pub async fn authorize_grant_plan(
        self,
        grants: &PluginWorkspaceGrantPlan,
    ) -> UseResult<RuntimeProviderSelection> {
        grants.validate()?;
        if grants.impact().scope_id != self.scope_id || !grants.impact().enabled_after {
            return Err(preflight_mismatch(
                "The workspace grant plan does not activate this Runtime preflight scope.",
            ));
        }
        let proposal = grants
            .change_set()
            .changes
            .binary_search_by(|change| change.package_id.cmp(&self.bundle.package_id))
            .ok()
            .and_then(|index| grants.change_set().changes.get(index))
            .and_then(|change| change.after.as_ref())
            .ok_or_else(|| {
                preflight_mismatch(
                    "The workspace grant plan has no candidate proposal for this Runtime package.",
                )
            })?;
        self.authorize(proposal).await
    }
}

fn preflight_mismatch(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.preflight_mismatch", message)
}
