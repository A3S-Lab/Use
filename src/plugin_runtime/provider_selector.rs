use std::collections::{BTreeMap, BTreeSet};

use a3s_runtime::{ProviderId, RuntimeClientRegistry};
use a3s_use_core::{PlanQualifiedSurfaceRef, PlannedProviderEvidence, UseError, UseResult};
use olpc_cjson::CanonicalFormatter;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::client::{
    enforcement_profile, runtime_capabilities_digest, runtime_error,
    validate_capabilities_for_plan, PluginRuntimeClient,
};
use super::model::{runtime_contract_error, RuntimeSurfacePlan};
use super::{RuntimeSurfacePlanKey, RuntimeSurfacePlanPublication};

const CONTROL_PROVIDER_SELECTION_SCHEMA: &str = "a3s.use.control-provider-selection.v1";
const MAX_CONTROL_PROVIDER_SELECTION_BYTES: usize = 16 * 1024;

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
    selection_digest: String,
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
    pub(crate) fn from_parts(
        plan: RuntimeSurfacePlan,
        provider: PlannedProviderEvidence,
        client: PluginRuntimeClient,
    ) -> UseResult<Self> {
        let selection_digest = provider_selection_digest(&provider)?;
        Ok(Self {
            plan,
            provider,
            client,
            selection_digest,
        })
    }

    pub fn plan(&self) -> &RuntimeSurfacePlan {
        &self.plan
    }

    pub fn provider(&self) -> &PlannedProviderEvidence {
        &self.provider
    }

    pub fn client(&self) -> &PluginRuntimeClient {
        &self.client
    }

    /// Digest of the canonical provider evidence descriptor used by the
    /// Control Store Runtime owner. Keeping it on the selected value lets a
    /// host publish the exact plan/key pair without reconstructing digest
    /// rules at another boundary.
    pub fn selection_digest(&self) -> &str {
        &self.selection_digest
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

    /// Convert every connected managed Runtime surface into the immutable
    /// publication consumed by [`RuntimeSurfacePlanStore`]. The resulting
    /// keys are derived from the plan context and the exact provider evidence;
    /// callers do not provide paths, generations, or digest fields manually.
    pub fn plan_publications(&self) -> UseResult<Vec<RuntimeSurfacePlanPublication>> {
        let mut publications = self
            .surfaces
            .iter()
            .map(|selected| {
                let key = RuntimeSurfacePlanKey::from_plan(selected.plan(), selected.provider())?;
                RuntimeSurfacePlanPublication::new(key, selected.plan().clone())
            })
            .collect::<UseResult<Vec<_>>>()?;
        publications.sort_by(|left, right| left.key.cmp(&right.key));
        Ok(publications)
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
            plan.validate()?;
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
            surfaces.push(SelectedRuntimeSurface::from_parts(
                plan,
                provider,
                client.clone(),
            )?);
        }
        Ok(RuntimeProviderSelection { surfaces })
    }
}

/// Compute the canonical digest shared by Runtime planning and the inactive
/// Control Store provider-selection projection. This is intentionally kept in
/// the Runtime boundary so a host cannot accidentally publish a key with a
/// second, subtly different digest algorithm.
pub(crate) fn provider_selection_digest(evidence: &PlannedProviderEvidence) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Descriptor<'a> {
        schema: &'static str,
        evidence: &'a PlannedProviderEvidence,
    }

    let descriptor = Descriptor {
        schema: CONTROL_PROVIDER_SELECTION_SCHEMA,
        evidence,
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    descriptor.serialize(&mut serializer).map_err(|error| {
        UseError::new(
            "use.plugin.runtime.provider_selection_invalid",
            format!("Failed to encode canonical Runtime provider evidence: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_PROVIDER_SELECTION_BYTES {
        return Err(UseError::new(
            "use.plugin.runtime.provider_selection_invalid",
            "Canonical Runtime provider evidence exceeds its size bound.",
        ));
    }
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
