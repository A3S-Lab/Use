use semver::Version;
use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::host::{validate_request_identity, verify_capabilities};
use super::validation::{strictly_sorted_unique, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginHostCapabilities,
    PluginManagedScope, PluginPackageId, PluginSurfaceRef, MAX_PLUGIN_PLAN_ITEMS,
};

pub const PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA: &str =
    "a3s.use.plugin-host-observation-request.v1";
pub const PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA: &str = "a3s.use.plugin-host-observation-result.v1";

const OBSERVATION_REQUEST_ERROR: &str = "use.plugin.host_observation_request_invalid";
const OBSERVATION_RESULT_ERROR: &str = "use.plugin.host_observation_result_invalid";

/// Use-owned desired package state. Cloud projects this value but never keeps
/// a second package lifecycle state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginDesiredState {
    Absent,
    InstalledDisabled,
    Enabled,
}

/// Use-owned aggregate observation produced by the Surface Reconciler.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginObservedState {
    Installed,
    Reconciling,
    Ready,
    Degraded,
    Broken,
    Incompatible,
    Draining,
    Removed,
}

/// Exact, non-secret package and capability evidence safe to project remotely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostPackageState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_digest: Option<String>,
    pub capability_generation: u64,
    pub capability_revision: String,
    pub desired: PluginDesiredState,
    pub observed: PluginObservedState,
    pub selected_surfaces: Vec<PluginSurfaceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginHostUnavailableReason {
    ManagedScopeUnavailable,
    ManagerRecovering,
    StateUnstable,
}

/// Observation availability is explicit so missing evidence cannot be
/// converted into success, absence, or a disabled flag.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "availability",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginHostObservationStatus {
    Available { state: PluginHostPackageState },
    Unavailable { reason: PluginHostUnavailableReason },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostObservationRequest {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHostObservationResult {
    pub schema: String,
    pub request_id: String,
    pub assignment_generation: u64,
    pub capabilities_digest: String,
    pub scope: PluginManagedScope,
    pub package_id: PluginPackageId,
    pub observed_at_ms: u64,
    pub status: PluginHostObservationStatus,
}

impl PluginHostPackageState {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_sha256(&self.capability_revision)
            || self.selected_surfaces.len() > MAX_PLUGIN_PLAN_ITEMS
            || !strictly_sorted_unique(&self.selected_surfaces)
        {
            return Err(package_state_error(
                "The plugin package generation, capability, or surface evidence is invalid.",
            ));
        }

        let present_fields = [
            self.version.is_some(),
            self.package_generation.is_some(),
            self.package_digest.is_some(),
            self.manifest_digest.is_some(),
            self.receipt_digest.is_some(),
        ];
        let present = present_fields.iter().all(|value| *value);
        if !present && present_fields.iter().any(|value| *value) {
            return Err(package_state_error(
                "Installed package evidence must be complete or wholly absent.",
            ));
        }
        if present {
            let version_is_canonical = self.version.as_deref().is_some_and(|value| {
                Version::parse(value).is_ok_and(|version| version.to_string() == value)
            });
            if !version_is_canonical
                || self.package_generation == Some(0)
                || self
                    .package_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_sha256(digest))
                || self
                    .manifest_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_sha256(digest))
                || self
                    .receipt_digest
                    .as_deref()
                    .is_some_and(|digest| !valid_sha256(digest))
                || self.selected_surfaces.is_empty()
            {
                return Err(package_state_error(
                    "Installed package identity, receipt, or selected surfaces are invalid.",
                ));
            }
        } else if !self.selected_surfaces.is_empty() {
            return Err(package_state_error(
                "An absent package cannot retain selected surface evidence.",
            ));
        }

        let lifecycle_is_valid = match self.desired {
            PluginDesiredState::Absent => match self.observed {
                PluginObservedState::Removed => !present,
                PluginObservedState::Draining => present,
                _ => false,
            },
            PluginDesiredState::InstalledDisabled => {
                present
                    && matches!(
                        self.observed,
                        PluginObservedState::Installed
                            | PluginObservedState::Reconciling
                            | PluginObservedState::Incompatible
                            | PluginObservedState::Draining
                    )
            }
            PluginDesiredState::Enabled => {
                present
                    && matches!(
                        self.observed,
                        PluginObservedState::Reconciling
                            | PluginObservedState::Ready
                            | PluginObservedState::Degraded
                            | PluginObservedState::Broken
                            | PluginObservedState::Incompatible
                            | PluginObservedState::Draining
                    )
            }
        };
        if !lifecycle_is_valid {
            return Err(package_state_error(
                "The desired and observed plugin lifecycle evidence is inconsistent.",
            ));
        }
        Ok(())
    }
}

impl PluginHostObservationStatus {
    pub fn validate(&self) -> UseResult<()> {
        match self {
            Self::Available { state } => state.validate(),
            Self::Unavailable { .. } => Ok(()),
        }
    }
}

impl PluginHostObservationRequest {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host observation request",
            OBSERVATION_REQUEST_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA {
            return Err(observation_request_error(
                "The plugin host observation request schema is unsupported.",
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
                "The plugin host observation request identity or scope is invalid.",
            )
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
            "plugin host observation request",
            OBSERVATION_REQUEST_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginHostObservationResult {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin host observation result",
            OBSERVATION_RESULT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA || self.observed_at_ms == 0 {
            return Err(observation_result_error(
                "The plugin host observation result schema or time is invalid.",
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
                "The plugin host observation result identity or scope is invalid.",
            )
        })?;
        self.status.validate().map_err(|_| {
            observation_result_error("The plugin host package observation is invalid.")
        })
    }

    pub fn validate_for(
        &self,
        request: &PluginHostObservationRequest,
        capabilities: &PluginHostCapabilities,
    ) -> UseResult<()> {
        self.validate()?;
        request.validate_for_capabilities(capabilities)?;
        verify_capabilities(&self.capabilities_digest, &self.scope, capabilities)?;
        if self.request_id != request.request_id
            || self.assignment_generation != request.assignment_generation
            || self.capabilities_digest != request.capabilities_digest
            || self.scope != request.scope
            || self.package_id != request.package_id
        {
            return Err(UseError::new(
                "use.plugin.host_observation_result_mismatch",
                "The plugin host observation result does not bind the exact request.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(
            self,
            "plugin host observation result",
            OBSERVATION_RESULT_ERROR,
        )
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn package_state_error(message: impl Into<String>) -> UseError {
    contract_error(OBSERVATION_RESULT_ERROR, message)
}

fn observation_request_error(message: impl Into<String>) -> UseError {
    contract_error(OBSERVATION_REQUEST_ERROR, message)
}

fn observation_result_error(message: impl Into<String>) -> UseError {
    contract_error(OBSERVATION_RESULT_ERROR, message)
}
