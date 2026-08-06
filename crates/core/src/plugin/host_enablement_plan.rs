use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::host::{validate_request_identity, verify_capabilities, verify_supported_plan_schema};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginDesiredState,
    PluginHostCapabilities, PluginHostPackageState, PluginHostPlanResult, PluginManagedScope,
    PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageId,
    PLUGIN_HOST_PLAN_RESULT_SCHEMA, PLUGIN_OPERATION_PLAN_SCHEMA_V4,
};

pub const PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA: &str =
    "a3s.use.plugin-host-enablement-plan-request.v1";
pub const PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA: &str =
    "a3s.use.plugin-host-enablement-plan-result.v1";

const ENABLEMENT_PLAN_REQUEST_ERROR: &str = "use.plugin.host_enablement_plan_request_invalid";
const ENABLEMENT_PLAN_RESULT_ERROR: &str = "use.plugin.host_enablement_plan_result_invalid";

/// Managed-scope request to inspect and plan one desired package state.
///
/// The manager owns the operation identity of any returned plan. A no-change
/// result is terminal and never invents a mutation plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostEnablementPlanRequest {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub expected_package_generation: u64,
    pub enabled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHostEnablementPlanStatus {
    NoChange,
    Planned,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostEnablementPlanResult {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub expected_package_generation: u64,
    pub enabled: bool,
    pub planned_at_ms: u64,
    pub status: PluginHostEnablementPlanStatus,
    pub state: PluginHostPackageState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PluginOperationPlanEnvelope>,
    pub replayed: bool,
}

impl PluginHostEnablementPlanRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host enablement plan request",
            ENABLEMENT_PLAN_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA
            || self.expected_package_generation == 0
        {
            return Err(enablement_plan_request_error());
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| enablement_plan_request_error())
    }

    pub fn validate_for_capabilities(
        &self,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;
        if !capabilities.supports_plan_schema(PLUGIN_OPERATION_PLAN_SCHEMA_V4) {
            return Err(UseError::new(
                "use.plugin.host_enablement_plan_unsupported",
                "The selected host protocol does not support reviewed enablement plans.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "plugin host enablement plan request",
            ENABLEMENT_PLAN_REQUEST_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostEnablementPlanResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host enablement plan result",
            ENABLEMENT_PLAN_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA
            || self.expected_package_generation == 0
            || self.planned_at_ms == 0
            || self.state.desired == PluginDesiredState::Absent
        {
            return Err(enablement_plan_result_error());
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| enablement_plan_result_error())?;
        self.state
            .validate()
            .map_err(|_| enablement_plan_result_error())?;
        if self.state.package_generation != Some(self.expected_package_generation) {
            return Err(enablement_plan_result_error());
        }

        let target = if self.enabled {
            PluginDesiredState::Enabled
        } else {
            PluginDesiredState::InstalledDisabled
        };
        match self.status {
            PluginHostEnablementPlanStatus::NoChange
                if self.state.desired == target && self.plan.is_none() =>
            {
                Ok(())
            }
            PluginHostEnablementPlanStatus::Planned if self.state.desired != target => self
                .validate_plan(
                    self.plan
                        .as_ref()
                        .ok_or_else(enablement_plan_result_error)?,
                ),
            _ => Err(enablement_plan_result_error()),
        }
    }

    pub fn validate_for(
        &self,
        request: &PluginHostEnablementPlanRequest,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate_for_capabilities(capabilities)?;
        request.validate_for_capabilities(capabilities)?;
        if self.request_id != request.request_id
            || self.assignment_generation != request.assignment_generation
            || self.capabilities_digest != request.capabilities_digest
            || self.scope != request.scope
            || self.package_id != request.package_id
            || self.expected_package_generation != request.expected_package_generation
            || self.enabled != request.enabled
        {
            return Err(UseError::new(
                "use.plugin.host_enablement_plan_result_mismatch",
                "The plugin host enablement plan result does not bind the exact request.",
            ));
        }
        Ok(())
    }

    pub fn validate_for_capabilities(
        &self,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;
        if let Some(plan) = &self.plan {
            verify_supported_plan_schema(capabilities, &plan.plan.schema)?;
        } else if !capabilities.supports_plan_schema(PLUGIN_OPERATION_PLAN_SCHEMA_V4) {
            return Err(UseError::new(
                "use.plugin.host_enablement_plan_unsupported",
                "The selected host protocol does not support reviewed enablement plans.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "plugin host enablement plan result",
            ENABLEMENT_PLAN_RESULT_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    /// Project the planned branch into the existing immutable host plan type so
    /// the established digest-only apply protocol remains the sole apply path.
    pub fn reviewed_plan(&self) -> UseResult<PluginHostPlanResult> {
        self.validate()?;
        if self.status != PluginHostEnablementPlanStatus::Planned {
            return Err(UseError::new(
                "use.plugin.host_enablement_no_change",
                "A no-change enablement outcome has no operation plan to apply.",
            ));
        }
        let result = PluginHostPlanResult {
            schema: PLUGIN_HOST_PLAN_RESULT_SCHEMA.to_string(),
            request_id: self.request_id.clone(),
            assignment_generation: self.assignment_generation,
            capabilities_digest: self.capabilities_digest.clone(),
            scope: self.scope.clone(),
            package_id: self.package_id.clone(),
            plan: self.plan.clone().ok_or_else(enablement_plan_result_error)?,
            replayed: self.replayed,
        };
        result.validate()?;
        Ok(result)
    }

    fn validate_plan(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        envelope
            .validate()
            .map_err(|_| enablement_plan_result_error())?;
        let action = if self.enabled {
            PluginOperationAction::Enable
        } else {
            PluginOperationAction::Disable
        };
        if envelope.plan.schema != PLUGIN_OPERATION_PLAN_SCHEMA_V4
            || envelope.plan.action != action
            || envelope.plan.package_id != self.package_id.as_str()
            || envelope.plan.scope != self.scope.plan_scope()
            || envelope.plan.created_at_ms != self.planned_at_ms
            || envelope.plan.state.receipt_digest != self.state.receipt_digest
            || envelope.plan.state.capability_generation != self.state.capability_generation
        {
            return Err(enablement_plan_result_error());
        }
        Ok(())
    }
}

fn enablement_plan_request_error() -> UseError {
    contract_error(
        ENABLEMENT_PLAN_REQUEST_ERROR,
        "The plugin host enablement plan request is invalid.",
    )
}

fn enablement_plan_result_error() -> UseError {
    contract_error(
        ENABLEMENT_PLAN_RESULT_ERROR,
        "The plugin host enablement plan result is invalid.",
    )
}
