use std::collections::{BTreeMap, BTreeSet};

use a3s_runtime::{ProviderId, RuntimeClientRegistry};
use a3s_use_core::{PlanQualifiedSurfaceRef, PlannedProviderEvidence, UseError, UseResult};

use super::client::{
    enforcement_profile, runtime_capabilities_digest, runtime_error,
    validate_capabilities_for_plan, PluginRuntimeClient,
};
use super::model::{runtime_contract_error, RuntimeSurfacePlan};

#[derive(Debug, Clone)]
pub struct RuntimeProviderAssignment {
    surface: PlanQualifiedSurfaceRef,
    provider_id: ProviderId,
}

impl RuntimeProviderAssignment {
    pub fn new(
        surface: PlanQualifiedSurfaceRef,
        provider_id: impl Into<String>,
    ) -> UseResult<Self> {
        let provider_id = ProviderId::parse(provider_id).map_err(|error| {
            UseError::new(
                "use.plugin.runtime.provider_invalid",
                format!("The explicit Runtime provider ID is invalid: {error}"),
            )
        })?;
        Ok(Self {
            surface,
            provider_id,
        })
    }

    pub fn surface(&self) -> &PlanQualifiedSurfaceRef {
        &self.surface
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }
}

#[derive(Clone)]
pub struct SelectedRuntimeSurface {
    plan: RuntimeSurfacePlan,
    provider: PlannedProviderEvidence,
    client: PluginRuntimeClient,
}

impl std::fmt::Debug for SelectedRuntimeSurface {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SelectedRuntimeSurface")
            .field("plan", &self.plan)
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl SelectedRuntimeSurface {
    pub fn plan(&self) -> &RuntimeSurfacePlan {
        &self.plan
    }

    pub fn provider(&self) -> &PlannedProviderEvidence {
        &self.provider
    }

    pub fn client(&self) -> &PluginRuntimeClient {
        &self.client
    }
}

#[derive(Debug, Clone, Default)]
pub struct RuntimeProviderSelection {
    surfaces: Vec<SelectedRuntimeSurface>,
}

impl RuntimeProviderSelection {
    pub fn surfaces(&self) -> &[SelectedRuntimeSurface] {
        &self.surfaces
    }

    pub fn provider_evidence(&self) -> Vec<PlannedProviderEvidence> {
        self.surfaces
            .iter()
            .map(|surface| surface.provider.clone())
            .collect()
    }
}

pub struct RuntimeProviderSelector<'a> {
    registry: &'a RuntimeClientRegistry,
}

impl<'a> RuntimeProviderSelector<'a> {
    pub fn new(registry: &'a RuntimeClientRegistry) -> Self {
        Self { registry }
    }

    /// Resolve only caller-assigned providers. There is no default provider
    /// and no fallback when one explicit assignment is unavailable.
    pub async fn select(
        &self,
        mut plans: Vec<RuntimeSurfacePlan>,
        mut assignments: Vec<RuntimeProviderAssignment>,
    ) -> UseResult<RuntimeProviderSelection> {
        plans.sort_by_key(RuntimeSurfacePlan::surface);
        assignments.sort_by(|left, right| left.surface.cmp(&right.surface));
        let duplicate_plan = plans
            .windows(2)
            .any(|pair| pair[0].surface() == pair[1].surface());
        let duplicate_assignment = assignments
            .windows(2)
            .any(|pair| pair[0].surface == pair[1].surface);
        let complete_assignment = plans.len() == assignments.len()
            && plans
                .iter()
                .zip(&assignments)
                .all(|(plan, assignment)| plan.surface() == assignment.surface);
        if duplicate_plan || duplicate_assignment || !complete_assignment {
            return Err(UseError::new(
                "use.plugin.runtime.provider_assignment_invalid",
                "Each executable plugin surface requires exactly one Runtime provider assignment.",
            ));
        }

        let provider_ids = assignments
            .iter()
            .map(|assignment| assignment.provider_id.clone())
            .collect::<BTreeSet<_>>();
        let mut providers = BTreeMap::new();
        for provider_id in provider_ids {
            let client = self
                .registry
                .connect(&provider_id)
                .await
                .map_err(|error| runtime_error("connect selected Runtime provider", error))?;
            let capabilities = client
                .capabilities()
                .await
                .map_err(|error| runtime_error("read selected Runtime capabilities", error))?;
            capabilities.validate().map_err(runtime_contract_error)?;
            let capability_digest = runtime_capabilities_digest(&capabilities)?;
            providers.insert(
                provider_id,
                (
                    PluginRuntimeClient::new(client),
                    capabilities,
                    capability_digest,
                ),
            );
        }

        let mut surfaces = Vec::with_capacity(assignments.len());
        for (plan, assignment) in plans.into_iter().zip(assignments) {
            let (client, capabilities, capability_digest) =
                providers.get(&assignment.provider_id).ok_or_else(|| {
                    UseError::new(
                        "use.plugin.runtime.provider_unavailable",
                        "The explicitly selected Runtime provider disappeared during resolution.",
                    )
                })?;
            validate_capabilities_for_plan(&plan, capabilities)?;
            let semantics_profile_digest = plan
                .spec()
                .semantics_profile_digest
                .clone()
                .ok_or_else(|| {
                    runtime_contract_error("Runtime plan omitted its semantics-profile digest.")
                })?;
            let provider = PlannedProviderEvidence {
                surface: plan.surface(),
                provider_id: capabilities.provider_id.to_string(),
                provider_build_id: capabilities.provider_build.clone(),
                capability_digest: capability_digest.clone(),
                semantics_profile_digest,
                enforcement: enforcement_profile(plan.spec().isolation)?,
            };
            surfaces.push(SelectedRuntimeSurface {
                plan,
                provider,
                client: client.clone(),
            });
        }
        Ok(RuntimeProviderSelection { surfaces })
    }
}
