use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::host::{validate_request_identity, verify_capabilities};
use super::validation::valid_sha256;
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanActor,
    PluginHostCapabilities, PluginManagedScope, PluginOperationPlan, PluginPackageId,
};

pub const PLUGIN_HOST_CANCEL_REQUEST_SCHEMA: &str = "a3s.use.plugin-host-cancel-request.v1";
pub const PLUGIN_HOST_CANCEL_RESULT_SCHEMA: &str = "a3s.use.plugin-host-cancel-result.v1";

const CANCEL_REQUEST_ERROR: &str = "use.plugin.host_cancel_request_invalid";
const CANCEL_RESULT_ERROR: &str = "use.plugin.host_cancel_result_invalid";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHostCancellationStatus {
    Cancelled,
    TooLate,
    AlreadyCompleted,
    AlreadyCancelled,
}

/// Exact explicit-user request to cancel one reviewed operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostCancelRequest {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub operation_id: String,
    pub plan_digest: String,
    pub requested_by: PlanActor,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostCancelResult {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub operation_id: String,
    pub plan_digest: String,
    pub observed_at_ms: u64,
    pub status: PluginHostCancellationStatus,
}

impl PluginHostCancelRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host cancellation request",
            CANCEL_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_CANCEL_REQUEST_SCHEMA || self.requested_by != PlanActor::User
        {
            return Err(cancel_request_error(
                "The cancellation schema or explicit user authority is invalid.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            cancel_request_error("The cancellation request identity or scope is invalid.")
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id)
            .map_err(|_| cancel_request_error("The cancelled operation identity is invalid."))?;
        if !valid_sha256(&self.plan_digest) {
            return Err(cancel_request_error(
                "The cancelled operation plan digest is invalid.",
            ));
        }
        Ok(())
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
            "plugin host cancellation request",
            CANCEL_REQUEST_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostCancelResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host cancellation result",
            CANCEL_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_CANCEL_RESULT_SCHEMA || self.observed_at_ms == 0 {
            return Err(cancel_result_error(
                "The cancellation result schema or observation time is invalid.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            cancel_result_error("The cancellation result identity or scope is invalid.")
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id)
            .map_err(|_| cancel_result_error("The cancelled operation identity is invalid."))?;
        if !valid_sha256(&self.plan_digest) {
            return Err(cancel_result_error(
                "The cancelled operation plan digest is invalid.",
            ));
        }
        Ok(())
    }

    pub fn validate_for(
        &self,
        request: &PluginHostCancelRequest,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        request.validate_for_capabilities(capabilities)?;
        if self.request_id != request.request_id
            || self.assignment_generation != request.assignment_generation
            || self.capabilities_digest != request.capabilities_digest
            || self.scope != request.scope
            || self.package_id != request.package_id
            || self.operation_id != request.operation_id
            || self.plan_digest != request.plan_digest
        {
            return Err(UseError::new(
                "use.plugin.host_cancel_result_mismatch",
                "The cancellation result does not bind the exact request.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin host cancellation result", CANCEL_RESULT_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn cancel_request_error(message: impl Into<String>) -> UseError {
    contract_error(CANCEL_REQUEST_ERROR, message)
}

fn cancel_result_error(message: impl Into<String>) -> UseError {
    contract_error(CANCEL_RESULT_ERROR, message)
}
