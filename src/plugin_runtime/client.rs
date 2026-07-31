use std::sync::Arc;

use a3s_runtime::contract::{
    IsolationLevel, RuntimeApplyRequest, RuntimeCapabilities, RuntimeFeature, RuntimeUnitClass,
};
use a3s_runtime::{RuntimeClient, RuntimeError};
use a3s_use_core::{PlanEnforcementProfile, PlannedProviderEvidence, UseError, UseResult};
use sha2::{Digest, Sha256};

use super::model::{
    runtime_contract_error, RuntimePreparedTaskBinding, RuntimeServiceActivation,
    RuntimeSurfaceContract, RuntimeSurfacePlan,
};

#[derive(Clone)]
pub struct PluginRuntimeClient {
    pub(super) client: Arc<dyn RuntimeClient>,
}

impl PluginRuntimeClient {
    pub fn new(client: Arc<dyn RuntimeClient>) -> Self {
        Self { client }
    }

    pub async fn verify_plan(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
    ) -> UseResult<RuntimeCapabilities> {
        validate_plan_evidence(plan, provider)?;
        let capabilities = self
            .client
            .capabilities()
            .await
            .map_err(|error| runtime_error("read Runtime capabilities", error))?;
        capabilities.validate().map_err(runtime_contract_error)?;

        let capability_digest = runtime_capabilities_digest(&capabilities)?;
        if capabilities.provider_id.as_str() != provider.provider_id
            || capabilities.provider_build != provider.provider_build_id
            || capability_digest != provider.capability_digest
        {
            return Err(UseError::new(
                "use.plugin.runtime.provider_evidence_changed",
                "The selected Runtime provider no longer matches the reviewed plan evidence.",
            )
            .with_detail("plannedProviderId", provider.provider_id.clone())
            .with_detail("observedProviderId", capabilities.provider_id.to_string())
            .with_detail("plannedProviderBuild", provider.provider_build_id.clone())
            .with_detail("observedProviderBuild", capabilities.provider_build.clone())
            .with_detail(
                "plannedCapabilityDigest",
                provider.capability_digest.clone(),
            )
            .with_detail("observedCapabilityDigest", capability_digest));
        }
        validate_capabilities_for_plan(plan, &capabilities)?;
        Ok(capabilities)
    }

    pub async fn prepare_task(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
    ) -> UseResult<RuntimePreparedTaskBinding> {
        if !matches!(plan.contract(), RuntimeSurfaceContract::ToolTask { .. })
            || plan.spec().class != RuntimeUnitClass::Task
        {
            return Err(UseError::new(
                "use.plugin.runtime.class_mismatch",
                "Only Runtime Task plans can produce prepared Task bindings.",
            ));
        }
        super::task::validate_task_capture_contract(plan.contract())?;
        self.verify_plan(plan, provider).await?;
        let semantics_profile_digest =
            plan.spec()
                .semantics_profile_digest
                .clone()
                .ok_or_else(|| {
                    runtime_contract_error(
                        "Runtime Task plan omitted its semantics-profile digest.",
                    )
                })?;
        Ok(RuntimePreparedTaskBinding {
            schema: super::model::RUNTIME_TASK_BINDING_SCHEMA.to_string(),
            surface: plan.surface(),
            package_digest: plan.context().package_digest().to_string(),
            scope_id: plan.context().scope_id().to_string(),
            descriptor_digest: plan.descriptor_digest().to_string(),
            provider_id: provider.provider_id.clone(),
            provider_build_id: provider.provider_build_id.clone(),
            capability_digest: provider.capability_digest.clone(),
            enforcement: provider.enforcement,
            artifact_digest: plan.spec().artifact.digest.clone(),
            artifact_media_type: plan.spec().artifact.media_type.clone(),
            generation: plan.spec().generation,
            semantics_profile_digest,
        })
    }

    pub async fn apply_service(
        &self,
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeServiceActivation> {
        if matches!(plan.contract(), RuntimeSurfaceContract::ToolTask { .. })
            || plan.spec().class != RuntimeUnitClass::Service
        {
            return Err(UseError::new(
                "use.plugin.runtime.class_mismatch",
                "Only Runtime Service plans can be applied as persistent plugin bindings.",
            ));
        }
        self.verify_plan(plan, provider).await?;
        let request = RuntimeApplyRequest {
            schema: RuntimeApplyRequest::SCHEMA.to_string(),
            request_id: request_id.into(),
            deadline_at_ms,
            spec: plan.spec().clone(),
        };
        request.validate().map_err(runtime_contract_error)?;
        let observation = self
            .client
            .apply(&request)
            .await
            .map_err(|error| runtime_error("apply Runtime Service", error))?;
        observation
            .validate_against(plan.spec())
            .map_err(runtime_contract_error)?;
        if observation.provider_build.as_deref() != Some(provider.provider_build_id.as_str()) {
            return Err(UseError::new(
                "use.plugin.runtime.observation_evidence_mismatch",
                "The Runtime Service observation was produced by an unreviewed provider build.",
            ));
        }
        if !observation.converges(plan.spec()) {
            return Err(UseError::new(
                "use.plugin.runtime.not_converged",
                "The Runtime Service did not reach its reviewed running and healthy state.",
            )
            .with_detail("unitId", observation.unit_id.clone())
            .with_detail(
                "state",
                serde_json::to_value(observation.state).unwrap_or_default(),
            ));
        }
        Ok(RuntimeServiceActivation {
            plan: plan.clone(),
            provider: provider.clone(),
            observation,
        })
    }
}

pub fn runtime_capabilities_digest(capabilities: &RuntimeCapabilities) -> UseResult<String> {
    capabilities.validate().map_err(runtime_contract_error)?;
    let mut canonical = capabilities.clone();
    canonical.unit_classes.sort();
    canonical.artifact_media_types.sort();
    canonical.isolation_levels.sort();
    canonical.network_modes.sort();
    canonical.mount_kinds.sort();
    canonical.health_check_kinds.sort();
    canonical.resource_controls.sort();
    canonical.features.sort();
    let bytes = serde_json::to_vec(&canonical).map_err(|error| {
        runtime_contract_error(format!(
            "Failed to encode canonical Runtime capabilities: {error}"
        ))
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn validate_plan_evidence(
    plan: &RuntimeSurfacePlan,
    provider: &PlannedProviderEvidence,
) -> UseResult<()> {
    let semantics_profile_digest = plan
        .spec()
        .semantics_profile_digest
        .as_deref()
        .ok_or_else(|| runtime_contract_error("Runtime plan omitted its semantics profile."))?;
    let expected_enforcement = enforcement_profile(plan.spec().isolation)?;
    if provider.surface != plan.surface()
        || provider.semantics_profile_digest != semantics_profile_digest
        || provider.enforcement != expected_enforcement
    {
        return Err(UseError::new(
            "use.plugin.runtime.plan_evidence_mismatch",
            "The Runtime surface spec does not match its reviewed provider evidence.",
        ));
    }
    Ok(())
}

pub(super) fn validate_capabilities_for_plan(
    plan: &RuntimeSurfacePlan,
    capabilities: &RuntimeCapabilities,
) -> UseResult<()> {
    capabilities.validate().map_err(runtime_contract_error)?;
    let mut missing = capabilities
        .missing_for(plan.spec())
        .map_err(runtime_contract_error)?;
    for feature in required_lifecycle_features(plan.contract()) {
        if !capabilities.supports_feature(feature) {
            missing.push(format!("feature:{feature:?}"));
        }
    }
    missing.sort();
    missing.dedup();
    if !missing.is_empty() {
        return Err(UseError::new(
            "use.plugin.runtime.capability_missing",
            "The selected Runtime provider cannot satisfy the reviewed surface plan.",
        )
        .with_detail("providerId", capabilities.provider_id.as_str().to_string())
        .with_detail(
            "missing",
            serde_json::to_value(&missing).unwrap_or_default(),
        ));
    }
    Ok(())
}

pub(super) fn enforcement_profile(isolation: IsolationLevel) -> UseResult<PlanEnforcementProfile> {
    match isolation {
        IsolationLevel::Container => Ok(PlanEnforcementProfile::Container),
        IsolationLevel::Sandbox => Ok(PlanEnforcementProfile::Sandbox),
        IsolationLevel::Process => Ok(PlanEnforcementProfile::NativeUnconfined),
        IsolationLevel::Confidential => Err(UseError::new(
            "use.plugin.runtime.enforcement_unsupported",
            "Confidential Runtime isolation is not representable in the plugin plan contract.",
        )),
    }
}

fn required_lifecycle_features(contract: &RuntimeSurfaceContract) -> Vec<RuntimeFeature> {
    match contract {
        RuntimeSurfaceContract::ToolTask { .. } => {
            vec![
                RuntimeFeature::Logs,
                RuntimeFeature::Stop,
                RuntimeFeature::Remove,
            ]
        }
        RuntimeSurfaceContract::ToolService { .. } | RuntimeSurfaceContract::McpService { .. } => {
            vec![RuntimeFeature::Stop, RuntimeFeature::Remove]
        }
    }
}

pub(super) fn runtime_error(action: &str, error: RuntimeError) -> UseError {
    let code = match error {
        RuntimeError::ProviderUnavailable(_) => "use.plugin.runtime.provider_unavailable",
        RuntimeError::UnsupportedCapabilities(_) => "use.plugin.runtime.capability_missing",
        RuntimeError::DeadlineExceeded(_) => "use.plugin.runtime.deadline_exceeded",
        _ => "use.plugin.runtime.operation_failed",
    };
    UseError::new(code, format!("Failed to {action}: {error}"))
}
