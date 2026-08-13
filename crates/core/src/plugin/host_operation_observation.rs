use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::host::{validate_request_identity, verify_capabilities};
use super::validation::{valid_machine_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginHostCapabilities,
    PluginHostPackageState, PluginManagedScope, PluginOperationPlan, PluginPackageId,
    PluginSurfaceRef,
};

pub const PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA: &str =
    "a3s.use.plugin-host-operation-observation-request.v1";
pub const PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA: &str =
    "a3s.use.plugin-host-operation-observation-result.v1";
pub const PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA: &str =
    "a3s.use.plugin-host-operation-watch-request.v1";

pub const MAX_PLUGIN_HOST_OPERATION_WATCH_TIMEOUT_MS: u64 = 30_000;

const OBSERVATION_REQUEST_ERROR: &str = "use.plugin.host_operation_observation_request_invalid";
const OBSERVATION_RESULT_ERROR: &str = "use.plugin.host_operation_observation_result_invalid";
const WATCH_REQUEST_ERROR: &str = "use.plugin.host_operation_watch_request_invalid";

/// Factual, product-safe phase for one exact reviewed operation.
///
/// This projection deliberately avoids percentages. A host may report a more
/// specific phase only when durable A3S Use evidence supports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHostOperationPhase {
    Planned,
    AwaitingConfirmation,
    Denied,
    Preparing,
    Publishing,
    Finalizing,
    Completed,
    Failed,
    Cancelled,
}

/// Exact cancellation boundary observed for an operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHostOperationCancellability {
    /// The operation has not crossed durable admission and can be cancelled.
    Cancellable,
    /// Cancellation is recorded and will be admitted at the next safe point.
    WaitingForSafePoint,
    /// Durable admission or capability publication has already started.
    TooLate,
    /// The operation is terminal or policy denied it.
    NotApplicable,
}

/// Bounded checkpoint projection. Counts are evidence, not a percentage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostOperationProgress {
    pub completed_steps: u32,
    pub total_steps: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_surface: Option<PluginSurfaceRef>,
}

/// Secret-free durable status for one exact operation and plan digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostOperationStatus {
    pub phase: PluginHostOperationPhase,
    pub cancellability: PluginHostOperationCancellability,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<PluginHostOperationProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_result_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state: Option<PluginHostPackageState>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostOperationObservationRequest {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub operation_id: String,
    pub plan_digest: String,
}

/// Long-poll request keyed by the last admitted status revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostOperationWatchRequest {
    pub schema: String,
    pub observation: PluginHostOperationObservationRequest,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_revision: Option<String>,
    pub timeout_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostOperationObservationResult {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub operation_id: String,
    pub plan_digest: String,
    pub observed_at_ms: u64,
    pub revision: String,
    pub changed: bool,
    pub timed_out: bool,
    pub status: PluginHostOperationStatus,
}

impl PluginHostOperationProgress {
    pub fn validate(&self) -> UseResult<()> {
        if self.total_steps == 0 || self.completed_steps > self.total_steps {
            return Err(observation_result_error(
                "The operation checkpoint progress is invalid.",
            ));
        }
        Ok(())
    }
}

impl PluginHostOperationStatus {
    pub fn validate(&self) -> UseResult<()> {
        if let Some(progress) = &self.progress {
            progress.validate()?;
        }
        if self
            .error_code
            .as_deref()
            .is_some_and(|code| !valid_machine_id(code))
            || self.completed_at_ms == Some(0)
            || self
                .operation_result_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
        {
            return Err(observation_result_error(
                "The operation status error, time, or result digest is invalid.",
            ));
        }
        if let Some(state) = &self.state {
            state.validate().map_err(|_| {
                observation_result_error("The operation status package state is invalid.")
            })?;
        }

        let terminal = matches!(
            self.phase,
            PluginHostOperationPhase::Denied
                | PluginHostOperationPhase::Completed
                | PluginHostOperationPhase::Failed
                | PluginHostOperationPhase::Cancelled
        );
        if terminal != (self.cancellability == PluginHostOperationCancellability::NotApplicable)
            || (self.phase == PluginHostOperationPhase::Failed) != self.error_code.is_some()
            || (self.phase == PluginHostOperationPhase::Completed)
                != (self.completed_at_ms.is_some()
                    && self.operation_result_digest.is_some()
                    && self.state.is_some())
            || (self.phase != PluginHostOperationPhase::Completed
                && (self.operation_result_digest.is_some() || self.state.is_some()))
            || (matches!(
                self.phase,
                PluginHostOperationPhase::Denied | PluginHostOperationPhase::Cancelled
            ) && self.completed_at_ms.is_none())
        {
            return Err(observation_result_error(
                "The operation phase is inconsistent with its terminal evidence.",
            ));
        }
        Ok(())
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(canonical_digest(&canonical_json(
            self,
            "plugin host operation status",
            OBSERVATION_RESULT_ERROR,
        )?))
    }
}

impl PluginHostOperationObservationRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host operation observation request",
            OBSERVATION_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA {
            return Err(observation_request_error(
                "The operation observation request schema is unsupported.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            observation_request_error(
                "The operation observation request identity or scope is invalid.",
            )
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id).map_err(|_| {
            observation_request_error("The observed operation identity is invalid.")
        })?;
        if !valid_sha256(&self.plan_digest) {
            return Err(observation_request_error(
                "The observed plan digest is invalid.",
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
            "plugin host operation observation request",
            OBSERVATION_REQUEST_ERROR,
        )
    }
}

impl PluginHostOperationWatchRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host operation watch request",
            WATCH_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        self.observation.validate()?;
        if self.schema != PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA
            || self.timeout_ms > MAX_PLUGIN_HOST_OPERATION_WATCH_TIMEOUT_MS
            || self
                .after_revision
                .as_deref()
                .is_some_and(|revision| !valid_sha256(revision))
        {
            return Err(watch_request_error(
                "The operation watch schema, revision, or timeout is invalid.",
            ));
        }
        Ok(())
    }

    pub fn validate_for_capabilities(
        &self,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        self.observation.validate_for_capabilities(capabilities)
    }
}

impl PluginHostOperationObservationResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host operation observation result",
            OBSERVATION_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA
            || self.observed_at_ms == 0
            || !valid_sha256(&self.revision)
            || self.timed_out && self.changed
        {
            return Err(observation_result_error(
                "The operation observation schema, time, revision, or watch result is invalid.",
            ));
        }
        validate_request_identity(
            &self.request_id,
            self.assignment_generation,
            &self.capabilities_digest,
            &self.scope,
        )
        .map_err(|_| {
            observation_result_error(
                "The operation observation result identity or scope is invalid.",
            )
        })?;
        PluginOperationPlan::validate_operation_id(&self.operation_id)
            .map_err(|_| observation_result_error("The observed operation identity is invalid."))?;
        if !valid_sha256(&self.plan_digest) {
            return Err(observation_result_error(
                "The observed operation plan digest is invalid.",
            ));
        }
        self.status.validate()?;
        if self.revision != self.status.descriptor_digest()? {
            return Err(observation_result_error(
                "The operation observation revision does not bind its exact status.",
            ));
        }
        Ok(())
    }

    pub fn validate_for(
        &self,
        request: &PluginHostOperationObservationRequest,
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
                "use.plugin.host_operation_observation_result_mismatch",
                "The operation observation does not bind the exact request.",
            ));
        }
        Ok(())
    }
}

fn observation_request_error(message: impl Into<String>) -> UseError {
    contract_error(OBSERVATION_REQUEST_ERROR, message)
}

fn observation_result_error(message: impl Into<String>) -> UseError {
    contract_error(OBSERVATION_RESULT_ERROR, message)
}

fn watch_request_error(message: impl Into<String>) -> UseError {
    contract_error(WATCH_REQUEST_ERROR, message)
}
