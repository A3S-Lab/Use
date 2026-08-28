use a3s_runtime::contract::{
    RuntimeHealthState, RuntimeObservation, RuntimeUnitClass, RuntimeUnitState,
};
use a3s_runtime::ProviderId;
use a3s_use_core::{
    PlanEnforcementProfile, PlanQualifiedSurfaceRef, PlanScope, PlannedProviderEvidence,
    PluginSurfaceKind, UseError, UseResult,
};
use serde::{Deserialize, Serialize};

use super::model::{
    runtime_contract_error, runtime_input_error, valid_machine_id, valid_sha256,
    RuntimeEndpointRef, RuntimeServiceBindingReceipt, RuntimeServiceReadinessEvidence,
    RuntimeSurfaceContract, RuntimeSurfacePlan, RUNTIME_SERVICE_BINDING_SCHEMA,
};
use super::receipt::RuntimeBindingReceipt;

pub const RUNTIME_SERVICE_PROVISIONING_SCHEMA: &str = "a3s.use.runtime-service-provisioning.v1";

/// Durable phase of one exact Runtime Service preparation.
///
/// The receipt is written before the Runtime apply call. Each later phase is
/// persisted before the package lifecycle checkpoint can complete, so a
/// process exit never leaves an unowned Runtime unit or Gateway binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeServiceProvisioningPhase {
    Requested,
    RuntimeApplied,
    GatewayReady,
}

/// Crash-recovery evidence for a Service that has not yet become a final
/// [`RuntimeServiceBindingReceipt`].
///
/// This record contains only immutable plan/provider identity and non-secret
/// Runtime/Gateway evidence. It is not capability publication authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RuntimeServiceProvisioningReceipt {
    pub schema: String,
    pub surface: PlanQualifiedSurfaceRef,
    pub package_digest: String,
    pub scope: PlanScope,
    pub grant_digest: String,
    pub descriptor_digest: String,
    pub provider_id: String,
    pub provider_build_id: String,
    pub capability_digest: String,
    pub enforcement: PlanEnforcementProfile,
    pub unit_id: String,
    pub generation: u64,
    pub spec_digest: String,
    pub semantics_profile_digest: String,
    pub contract: RuntimeSurfaceContract,
    pub lifecycle_idempotency_key: String,
    pub apply_request_id: String,
    pub phase: RuntimeServiceProvisioningPhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<RuntimeObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<RuntimeEndpointRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readiness: Option<RuntimeServiceReadinessEvidence>,
}

impl RuntimeServiceProvisioningReceipt {
    pub fn from_plan(
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
        lifecycle_idempotency_key: impl Into<String>,
        apply_request_id: impl Into<String>,
    ) -> UseResult<Self> {
        plan.spec().validate().map_err(runtime_contract_error)?;
        if plan.spec().class != RuntimeUnitClass::Service
            || matches!(plan.contract(), RuntimeSurfaceContract::ToolTask { .. })
            || provider.surface != plan.surface()
        {
            return Err(runtime_input_error(
                "Only an exact selected Runtime Service plan can start provisioning.",
            ));
        }
        let semantics_profile_digest = plan
            .spec()
            .semantics_profile_digest
            .clone()
            .ok_or_else(|| runtime_contract_error("Runtime plan omitted its semantics profile."))?;
        if provider.semantics_profile_digest != semantics_profile_digest {
            return Err(runtime_input_error(
                "Runtime provisioning provider evidence does not match the plan semantics.",
            ));
        }
        if provider.enforcement != super::client::enforcement_profile(plan.spec().isolation)? {
            return Err(runtime_input_error(
                "Runtime provisioning enforcement evidence does not match the plan isolation.",
            ));
        }
        let receipt = Self {
            schema: RUNTIME_SERVICE_PROVISIONING_SCHEMA.to_owned(),
            surface: plan.surface(),
            package_digest: plan.context().package_digest().to_owned(),
            scope: plan.context().scope().clone(),
            grant_digest: plan.context().grant_digest().to_owned(),
            descriptor_digest: plan.descriptor_digest().to_owned(),
            provider_id: provider.provider_id.clone(),
            provider_build_id: provider.provider_build_id.clone(),
            capability_digest: provider.capability_digest.clone(),
            enforcement: provider.enforcement,
            unit_id: plan.spec().unit_id.clone(),
            generation: plan.context().generation(),
            spec_digest: plan.spec().digest().map_err(runtime_contract_error)?,
            semantics_profile_digest,
            contract: plan.contract().clone(),
            lifecycle_idempotency_key: lifecycle_idempotency_key.into(),
            apply_request_id: apply_request_id.into(),
            phase: RuntimeServiceProvisioningPhase::Requested,
            observation: None,
            endpoint_ref: None,
            readiness: None,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> UseResult<()> {
        let valid_shape = self.schema == RUNTIME_SERVICE_PROVISIONING_SCHEMA
            && valid_binding_identity(&self.surface, &self.scope)
            && valid_sha256(&self.package_digest)
            && valid_sha256(&self.grant_digest)
            && valid_sha256(&self.descriptor_digest)
            && valid_sha256(&self.capability_digest)
            && valid_sha256(&self.spec_digest)
            && valid_sha256(&self.semantics_profile_digest)
            && ProviderId::parse(&self.provider_id).is_ok()
            && valid_machine_id(&self.provider_build_id)
            && valid_runtime_unit_id(&self.unit_id)
            && valid_sha256(&self.lifecycle_idempotency_key)
            && valid_machine_id(&self.apply_request_id)
            && self.generation > 0;
        if !valid_shape || !contract_matches_surface(&self.surface, &self.contract) {
            return Err(runtime_input_error(
                "The Runtime Service provisioning receipt is invalid.",
            ));
        }

        match self.phase {
            RuntimeServiceProvisioningPhase::Requested => {
                if self.observation.is_some()
                    || self.endpoint_ref.is_some()
                    || self.readiness.is_some()
                {
                    return Err(invalid_phase());
                }
            }
            RuntimeServiceProvisioningPhase::RuntimeApplied => {
                let observation = self.observation.as_ref().ok_or_else(invalid_phase)?;
                validate_observation(self, observation)?;
                if self.endpoint_ref.is_some() || self.readiness.is_some() {
                    return Err(invalid_phase());
                }
            }
            RuntimeServiceProvisioningPhase::GatewayReady => {
                let observation = self.observation.as_ref().ok_or_else(invalid_phase)?;
                validate_observation(self, observation)?;
                if self.endpoint_ref.is_none() || self.readiness.is_none() {
                    return Err(invalid_phase());
                }
                self.binding_receipt()?;
            }
        }
        Ok(())
    }

    pub fn matches_plan(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
        lifecycle_idempotency_key: &str,
        apply_request_id: &str,
    ) -> UseResult<bool> {
        self.validate()?;
        let spec_digest = plan.spec().digest().map_err(runtime_contract_error)?;
        Ok(self.surface == plan.surface()
            && self.package_digest == plan.context().package_digest()
            && self.scope == *plan.context().scope()
            && self.grant_digest == plan.context().grant_digest()
            && self.descriptor_digest == plan.descriptor_digest()
            && self.provider_id == provider.provider_id
            && self.provider_build_id == provider.provider_build_id
            && self.capability_digest == provider.capability_digest
            && self.enforcement == provider.enforcement
            && self.unit_id == plan.spec().unit_id
            && self.generation == plan.context().generation()
            && self.spec_digest == spec_digest
            && self.semantics_profile_digest == provider.semantics_profile_digest
            && self.contract == *plan.contract()
            && self.lifecycle_idempotency_key == lifecycle_idempotency_key
            && self.apply_request_id == apply_request_id)
    }

    pub fn record_runtime_observation(
        &mut self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
        observation: RuntimeObservation,
    ) -> UseResult<()> {
        if self.phase == RuntimeServiceProvisioningPhase::GatewayReady
            || !self.matches_plan(
                plan,
                provider,
                &self.lifecycle_idempotency_key,
                &self.apply_request_id,
            )?
        {
            return Err(runtime_input_error(
                "Runtime apply evidence does not belong to the pending provisioning request.",
            ));
        }
        observation
            .validate_against(plan.spec())
            .map_err(runtime_contract_error)?;
        if !observation.converges(plan.spec())
            || observation.provider_build.as_deref() != Some(self.provider_build_id.as_str())
        {
            return Err(runtime_input_error(
                "Runtime provisioning did not return the exact healthy Service generation.",
            ));
        }
        if let Some(current) = &self.observation {
            if observation.observed_at_ms < current.observed_at_ms
                || observation.started_at_ms != current.started_at_ms
                || observation.provider_resource_id != current.provider_resource_id
            {
                return Err(runtime_input_error(
                    "Runtime provisioning observation identity regressed during recovery.",
                ));
            }
        }
        self.phase = RuntimeServiceProvisioningPhase::RuntimeApplied;
        self.observation = Some(observation);
        self.endpoint_ref = None;
        self.readiness = None;
        self.validate()
    }

    pub fn record_gateway_readiness(
        &mut self,
        endpoint_ref: RuntimeEndpointRef,
        readiness: RuntimeServiceReadinessEvidence,
    ) -> UseResult<()> {
        if self.phase != RuntimeServiceProvisioningPhase::RuntimeApplied {
            return Err(invalid_phase());
        }
        let mut candidate = self.clone();
        candidate.phase = RuntimeServiceProvisioningPhase::GatewayReady;
        candidate.endpoint_ref = Some(endpoint_ref);
        candidate.readiness = Some(readiness);
        candidate.binding_receipt()?;
        *self = candidate;
        Ok(())
    }

    pub fn binding_receipt(&self) -> UseResult<RuntimeServiceBindingReceipt> {
        if self.phase != RuntimeServiceProvisioningPhase::GatewayReady {
            return Err(invalid_phase());
        }
        let observation = self.observation.as_ref().ok_or_else(invalid_phase)?;
        let endpoint_ref = self.endpoint_ref.clone().ok_or_else(invalid_phase)?;
        let readiness = self.readiness.clone().ok_or_else(invalid_phase)?;
        let runtime_started_at_ms = observation.started_at_ms.ok_or_else(invalid_phase)?;
        let last_healthy_at_ms = observation
            .health
            .as_ref()
            .map_or(observation.observed_at_ms, |health| health.checked_at_ms);
        let receipt = RuntimeServiceBindingReceipt {
            schema: RUNTIME_SERVICE_BINDING_SCHEMA.to_owned(),
            surface: self.surface.clone(),
            package_digest: self.package_digest.clone(),
            scope: self.scope.clone(),
            descriptor_digest: self.descriptor_digest.clone(),
            provider_id: self.provider_id.clone(),
            provider_build_id: self.provider_build_id.clone(),
            capability_digest: self.capability_digest.clone(),
            enforcement: self.enforcement,
            unit_id: self.unit_id.clone(),
            generation: self.generation,
            spec_digest: self.spec_digest.clone(),
            semantics_profile_digest: self.semantics_profile_digest.clone(),
            endpoint_ref,
            runtime_started_at_ms,
            observation_revision: observation.observed_at_ms,
            last_healthy_at_ms,
            contract: self.contract.clone(),
            readiness,
        };
        RuntimeBindingReceipt::Service(receipt.clone()).validate()?;
        Ok(receipt)
    }
}

fn validate_observation(
    receipt: &RuntimeServiceProvisioningReceipt,
    observation: &RuntimeObservation,
) -> UseResult<()> {
    observation.validate().map_err(runtime_contract_error)?;
    let evidence_matches = observation.evidence.as_ref().is_none_or(|evidence| {
        evidence.spec_digest == receipt.spec_digest
            && evidence.semantics_profile_digest.as_deref()
                == Some(receipt.semantics_profile_digest.as_str())
    });
    if observation.unit_id != receipt.unit_id
        || observation.generation != receipt.generation
        || observation.spec_digest != receipt.spec_digest
        || observation.class != RuntimeUnitClass::Service
        || observation.state != RuntimeUnitState::Running
        || observation.provider_build.as_deref() != Some(receipt.provider_build_id.as_str())
        || observation.started_at_ms.is_none()
        || observation.health.as_ref().is_none_or(|health| {
            health.state != RuntimeHealthState::Healthy
                || health.checked_at_ms == 0
                || health.checked_at_ms > observation.observed_at_ms
        })
        || !evidence_matches
    {
        return Err(runtime_input_error(
            "The Runtime Service provisioning observation is invalid.",
        ));
    }
    Ok(())
}

fn valid_binding_identity(surface: &PlanQualifiedSurfaceRef, scope: &PlanScope) -> bool {
    let package_segments = surface.package_id.split('/').collect::<Vec<_>>();
    surface.package_id.len() <= 128
        && package_segments.len() == 2
        && package_segments
            .iter()
            .all(|segment| super::model::valid_surface_segment(segment))
        && super::model::valid_surface_segment(&surface.surface.id)
        && scope.validate().is_ok()
}

fn valid_runtime_unit_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 512
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
}

fn contract_matches_surface(
    surface: &PlanQualifiedSurfaceRef,
    contract: &RuntimeSurfaceContract,
) -> bool {
    matches!(
        (surface.surface.kind, contract),
        (
            PluginSurfaceKind::Tool,
            RuntimeSurfaceContract::ToolService { .. }
        ) | (
            PluginSurfaceKind::Mcp,
            RuntimeSurfaceContract::McpService { .. }
        )
    )
}

fn invalid_phase() -> UseError {
    runtime_input_error("The Runtime Service provisioning phase evidence is inconsistent.")
}
