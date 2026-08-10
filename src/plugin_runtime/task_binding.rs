use a3s_runtime::contract::RuntimeUnitClass;
use a3s_runtime::ProviderId;
use a3s_use_core::{PlannedProviderEvidence, PluginSurfaceKind, UseResult};

use super::client::enforcement_profile;
use super::model::{
    runtime_contract_error, runtime_input_error, valid_machine_id, valid_sha256,
    RuntimePreparedTaskBinding, RuntimeSurfaceContext, RuntimeSurfaceContract, RuntimeSurfacePlan,
    RuntimeTaskInvocation, RUNTIME_TASK_BINDING_SCHEMA,
};
use super::planner::{runtime_semantics_profile_digest, runtime_unit_id};
use super::task::validate_task_capture_contract;

impl RuntimePreparedTaskBinding {
    pub(crate) fn from_plan(
        plan: &RuntimeSurfacePlan,
        provider: &PlannedProviderEvidence,
    ) -> UseResult<Self> {
        validate_task_capture_contract(plan.contract())?;
        let semantics_profile_digest =
            plan.spec()
                .semantics_profile_digest
                .clone()
                .ok_or_else(|| {
                    runtime_contract_error(
                        "Runtime Task plan omitted its semantics-profile digest.",
                    )
                })?;
        let mut template_spec = plan.spec().clone();
        template_spec.unit_id = runtime_unit_id(plan.context(), "task-template", None)?;
        template_spec.process.args.clear();
        template_spec.validate().map_err(runtime_contract_error)?;
        let binding = Self {
            schema: RUNTIME_TASK_BINDING_SCHEMA.to_string(),
            surface: plan.surface(),
            package_digest: plan.context().package_digest().to_string(),
            scope: plan.context().scope().clone(),
            grant_digest: plan.context().grant_digest().to_string(),
            descriptor_digest: plan.descriptor_digest().to_string(),
            provider_id: provider.provider_id.clone(),
            provider_build_id: provider.provider_build_id.clone(),
            capability_digest: provider.capability_digest.clone(),
            enforcement: provider.enforcement,
            semantics_profile_digest,
            template_spec: Box::new(template_spec),
            contract: plan.contract().clone(),
        };
        binding.validate()?;
        Ok(binding)
    }

    /// Rebuild one invocation-specific Runtime plan from the durable template.
    ///
    /// Invocation IDs and arguments are deliberately absent from the receipt.
    /// They produce a unique Runtime unit while the reviewed semantics profile,
    /// provider evidence, artifact, resources, and authority remain unchanged.
    pub fn invocation_plan(
        &self,
        invocation: RuntimeTaskInvocation,
    ) -> UseResult<RuntimeSurfacePlan> {
        self.validate()?;
        let context = self.context()?;
        let mut spec = self.template_spec.as_ref().clone();
        spec.unit_id = runtime_unit_id(&context, "task", Some(invocation.invocation_id()))?;
        spec.process.args = invocation.args;
        spec.validate().map_err(runtime_contract_error)?;
        Ok(RuntimeSurfacePlan {
            context,
            descriptor_digest: self.descriptor_digest.clone(),
            spec,
            contract: self.contract.clone(),
        })
    }

    pub fn generation(&self) -> u64 {
        self.template_spec.generation
    }

    pub fn validate(&self) -> UseResult<()> {
        let context = self.context()?;
        self.template_spec
            .validate()
            .map_err(runtime_contract_error)?;
        let expected_unit_id = runtime_unit_id(&context, "task-template", None)?;
        let expected_semantics = runtime_semantics_profile_digest(
            &context,
            &self.descriptor_digest,
            self.template_spec.as_ref(),
            &self.contract,
        )?;
        if self.schema != RUNTIME_TASK_BINDING_SCHEMA
            || self.surface.surface.kind != PluginSurfaceKind::Tool
            || !valid_sha256(&self.descriptor_digest)
            || !valid_sha256(&self.capability_digest)
            || !valid_sha256(&self.semantics_profile_digest)
            || ProviderId::parse(self.provider_id.as_str()).is_err()
            || !valid_machine_id(&self.provider_build_id)
            || self.template_spec.class != RuntimeUnitClass::Task
            || self.template_spec.unit_id != expected_unit_id
            || !self.template_spec.process.args.is_empty()
            || self.template_spec.semantics_profile_digest.as_deref()
                != Some(self.semantics_profile_digest.as_str())
            || expected_semantics != self.semantics_profile_digest
            || enforcement_profile(self.template_spec.isolation)? != self.enforcement
            || !matches!(self.contract, RuntimeSurfaceContract::ToolTask { .. })
        {
            return Err(runtime_input_error(
                "The prepared Runtime Task binding receipt is invalid.",
            ));
        }
        validate_task_capture_contract(&self.contract)?;
        Ok(())
    }

    fn context(&self) -> UseResult<RuntimeSurfaceContext> {
        RuntimeSurfaceContext::new(
            self.surface.package_id.clone(),
            self.package_digest.clone(),
            self.scope.clone(),
            self.grant_digest.clone(),
            self.surface.surface.clone(),
            self.template_spec.generation,
        )
    }
}
