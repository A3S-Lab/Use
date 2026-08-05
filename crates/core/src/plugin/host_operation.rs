use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::host::{validate_request_identity, verify_capabilities, verify_supported_plan_schema};
use super::validation::valid_sha256;
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanPolicyDecision,
    PluginDesiredState, PluginHostCapabilities, PluginHostPackageState, PluginHostPlanResult,
    PluginManagedScope, PluginOperationConfirmation, PluginOperationPlan, PluginPackageId,
};

pub const PLUGIN_HOST_APPLY_REQUEST_SCHEMA: &str = "a3s.use.plugin-host-apply-request.v1";
pub const PLUGIN_HOST_APPLY_RESULT_SCHEMA: &str = "a3s.use.plugin-host-apply-result.v1";
pub const PLUGIN_HOST_ENABLEMENT_REQUEST_SCHEMA: &str = "a3s.use.plugin-host-enablement-request.v1";
pub const PLUGIN_HOST_ENABLEMENT_RESULT_SCHEMA: &str = "a3s.use.plugin-host-enablement-result.v1";

const APPLY_REQUEST_ERROR: &str = "use.plugin.host_apply_request_invalid";
const APPLY_RESULT_ERROR: &str = "use.plugin.host_apply_result_invalid";
const ENABLEMENT_REQUEST_ERROR: &str = "use.plugin.host_enablement_request_invalid";
const ENABLEMENT_RESULT_ERROR: &str = "use.plugin.host_enablement_result_invalid";

/// Digest-only apply request for a plan already stored by the shared manager.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostApplyRequest {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub operation_id: String,
    pub plan_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation: Option<PluginOperationConfirmation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostApplyResult {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub operation_id: String,
    pub plan_digest: String,
    pub completed_at_ms: u64,
    pub operation_result_digest: String,
    pub state: PluginHostPackageState,
    pub replayed: bool,
}

/// Idempotent desired enablement change for one installed package generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostEnablementRequest {
    pub schema: String,
    pub request_id: String,
    pub operation_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub expected_package_generation: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostEnablementResult {
    pub schema: String,
    pub request_id: String,
    pub operation_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub completed_at_ms: u64,
    pub operation_result_digest: String,
    pub changed: bool,
    pub state: PluginHostPackageState,
    pub replayed: bool,
}

impl PluginHostApplyRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host apply request",
            APPLY_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_APPLY_REQUEST_SCHEMA {
            return Err(apply_request_error(
                "The plugin host apply request schema is unsupported.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            apply_request_error("The plugin host apply request identity or scope is invalid.")
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            apply_request_error("The plugin host apply operation identity is invalid.")
        })?;
        if !valid_sha256(&self.plan_digest) {
            return Err(apply_request_error(
                "The plugin host apply request plan digest is invalid.",
            ));
        }
        if let Some(confirmation) = &self.confirmation {
            confirmation.validate().map_err(|_| {
                apply_request_error("The plugin host apply confirmation is invalid.")
            })?;
            if confirmation.operation_id != self.operation_id
                || confirmation.plan_digest != self.plan_digest
            {
                return Err(apply_request_error(
                    "The confirmation does not bind the requested operation and plan digest.",
                ));
            }
        }
        Ok(())
    }

    pub fn validate_for_plan(
        &self,
        plan: &PluginHostPlanResult,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate_for_capabilities(capabilities)?;
        plan.validate()?;
        verify_supported_plan_schema(capabilities, &plan.plan.plan.schema)?;
        if self.assignment_generation != plan.assignment_generation
            || self.capabilities_digest != plan.capabilities_digest
            || self.scope != plan.scope
            || self.package_id != plan.package_id
            || self.operation_id != plan.plan.plan.operation_id
            || self.plan_digest != plan.plan.plan_digest
        {
            return Err(UseError::new(
                "use.plugin.host_apply_request_mismatch",
                "The plugin host apply request does not bind the exact reviewed plan.",
            ));
        }
        match (plan.plan.plan.authority.decision, &self.confirmation) {
            (PlanPolicyDecision::Allow, None) | (PlanPolicyDecision::Ask, Some(_)) => Ok(()),
            (PlanPolicyDecision::Deny, _) => Err(UseError::new(
                "use.plugin.plan_denied",
                "Policy denies applying the plugin operation plan.",
            )),
            _ => Err(UseError::new(
                "use.plugin.plan_confirmation_mismatch",
                "The apply request confirmation does not match the plan's policy decision.",
            )),
        }
    }

    /// Validate the request against the exact host contract selected by its
    /// capability digest and managed-scope fence.
    pub fn validate_for_capabilities(
        &self,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)
    }

    /// Perform the canonical lifetime and confirmation check at the host clock.
    pub fn verify_apply_for_plan(
        &self,
        plan: &PluginHostPlanResult,
        capabilities: &PluginHostCapabilities,
        now_ms: u64,
    ) -> UseResult<()> {
        self.validate_for_plan(plan, capabilities)?;
        plan.plan.verify_confirmed_apply(
            &self.operation_id,
            &self.plan_digest,
            self.confirmation.as_ref(),
            now_ms,
        )
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin host apply request", APPLY_REQUEST_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostApplyResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host apply result",
            APPLY_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_APPLY_RESULT_SCHEMA
            || self.completed_at_ms == 0
            || !valid_sha256(&self.operation_result_digest)
            || !valid_sha256(&self.plan_digest)
        {
            return Err(apply_result_error(
                "The plugin host apply result schema, time, or digest is invalid.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            apply_result_error("The plugin host apply result identity or scope is invalid.")
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            apply_result_error("The plugin host apply result operation identity is invalid.")
        })?;
        self.state
            .validate()
            .map_err(|_| apply_result_error("The applied plugin state is invalid."))
    }

    pub fn validate_for(
        &self,
        request: &PluginHostApplyRequest,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        request.validate()?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;
        if self.request_id != request.request_id
            || self.assignment_generation != request.assignment_generation
            || self.capabilities_digest != request.capabilities_digest
            || self.scope != request.scope
            || self.package_id != request.package_id
            || self.operation_id != request.operation_id
            || self.plan_digest != request.plan_digest
        {
            return Err(UseError::new(
                "use.plugin.host_apply_result_mismatch",
                "The plugin host apply result does not bind the exact request.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin host apply result", APPLY_RESULT_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostEnablementRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host enablement request",
            ENABLEMENT_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_ENABLEMENT_REQUEST_SCHEMA
            || self.expected_package_generation == 0
        {
            return Err(enablement_request_error(
                "The plugin host enablement request schema or generation is invalid.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            enablement_request_error(
                "The plugin host enablement request identity or scope is invalid.",
            )
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            enablement_request_error("The plugin host enablement operation identity is invalid.")
        })
    }

    pub fn validate_for_capabilities(
        &self,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "plugin host enablement request",
            ENABLEMENT_REQUEST_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostEnablementResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host enablement result",
            ENABLEMENT_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_ENABLEMENT_RESULT_SCHEMA
            || self.completed_at_ms == 0
            || !valid_sha256(&self.operation_result_digest)
        {
            return Err(enablement_result_error(
                "The plugin host enablement result schema, time, or digest is invalid.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            enablement_result_error(
                "The plugin host enablement result identity or scope is invalid.",
            )
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            enablement_result_error("The plugin host enablement operation identity is invalid.")
        })?;
        self.state
            .validate()
            .map_err(|_| enablement_result_error("The enabled plugin state is invalid."))?;
        if self.state.desired == PluginDesiredState::Absent {
            return Err(enablement_result_error(
                "Enablement cannot return an absent desired package state.",
            ));
        }
        Ok(())
    }

    pub fn validate_for(
        &self,
        request: &PluginHostEnablementRequest,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        request.validate_for_capabilities(capabilities)?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;
        let expected_desired = if request.enabled {
            PluginDesiredState::Enabled
        } else {
            PluginDesiredState::InstalledDisabled
        };
        let generation = self.state.package_generation.ok_or_else(|| {
            UseError::new(
                "use.plugin.host_enablement_result_mismatch",
                "Enablement did not return an installed package generation.",
            )
        })?;
        let generation_matches = if self.changed {
            generation > request.expected_package_generation
        } else {
            generation == request.expected_package_generation
        };
        if self.request_id != request.request_id
            || self.operation_id != request.operation_id
            || self.assignment_generation != request.assignment_generation
            || self.capabilities_digest != request.capabilities_digest
            || self.scope != request.scope
            || self.package_id != request.package_id
            || self.state.desired != expected_desired
            || !generation_matches
        {
            return Err(UseError::new(
                "use.plugin.host_enablement_result_mismatch",
                "The plugin host enablement result does not bind the exact request and generation.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "plugin host enablement result",
            ENABLEMENT_RESULT_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn apply_request_error(message: impl Into<String>) -> UseError {
    contract_error(APPLY_REQUEST_ERROR, message)
}

fn apply_result_error(message: impl Into<String>) -> UseError {
    contract_error(APPLY_RESULT_ERROR, message)
}

fn enablement_request_error(message: impl Into<String>) -> UseError {
    contract_error(ENABLEMENT_REQUEST_ERROR, message)
}

fn enablement_result_error(message: impl Into<String>) -> UseError {
    contract_error(ENABLEMENT_RESULT_ERROR, message)
}
