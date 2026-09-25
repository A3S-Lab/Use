//! Enablement projection types retained for Control diagnostics and planning.
//!
//! The legacy `package-enablement/` file store has been removed. Production
//! enable/disable mutates only through Control Store.

use a3s_use_core::{
    InstallationSnapshot, PlanPackageChangeKind, PlanScope, PluginOperationAction,
    PluginOperationPlanEnvelope, PluginPackageId, UseError, UseResult,
};
use serde::{Deserialize, Serialize};

use crate::plugin_lifecycle::{PluginLifecycleAction, PluginLifecycleIntent};

use super::grant::PackageGraphAuthorization;
use super::{CognitivePackageEnablementRequest, CognitivePackageEnablementResult};

pub(super) const ENABLEMENT_STATE_SCHEMA: &str =
    "a3s.use.cognitive-package-enablement-projection.v3";
const ENABLEMENT_OPERATION_SCHEMA: &str = "a3s.use.cognitive-package-enablement-operation.v3";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct CognitivePackageArtifactState {
    pub version: String,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
}

impl CognitivePackageArtifactState {
    pub fn validate(&self) -> UseResult<()> {
        if self.version.is_empty()
            || self.version.len() > 256
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
        {
            return Err(store_invalid(
                "A cognitive-package enablement artifact identity is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PendingCognitivePackageEnablement {
    pub request_digest: String,
    pub request: CognitivePackageEnablementRequest,
    pub intent: PluginLifecycleIntent,
    pub envelope: PluginOperationPlanEnvelope,
    pub authorization: PackageGraphAuthorization,
    pub state_generation_after: u64,
    pub started_at_ms: u64,
}

impl PendingCognitivePackageEnablement {
    fn validate_against(&self, state: &StoredCognitivePackageEnablement) -> UseResult<()> {
        self.request.validate()?;
        self.intent.validate()?;
        let artifact = state.artifact.as_ref().ok_or_else(|| {
            store_invalid("An absent cognitive package cannot retain an enablement operation.")
        })?;
        let expected_action = if self.request.enabled {
            PluginLifecycleAction::Enable
        } else {
            PluginLifecycleAction::Disable
        };
        if !valid_sha256(&self.request_digest)
            || self.request.descriptor_digest()? != self.request_digest
            || self.request.package_id.as_str() != state.package_id
            || self.request.expected_package_generation != state.state_generation
            || self.request.enabled == state.enabled
            || self.state_generation_after <= state.state_generation
            || self.started_at_ms == 0
            || self.intent.operation_id != self.request.operation_id
            || self.intent.scope != state.scope
            || self.intent.package_id != state.package_id
            || self.intent.package_digest != artifact.package_digest
            || self.intent.manifest_digest != artifact.manifest_digest
            || self.intent.generation != artifact.generation
            || self.intent.action != expected_action
        {
            return Err(store_invalid(
                "A pending cognitive-package enablement operation is invalid.",
            ));
        }
        self.envelope.validate()?;
        self.authorization
            .validate_against(&self.envelope, self.started_at_ms)?;
        let expected_plan_action = if self.request.enabled {
            PluginOperationAction::Enable
        } else {
            PluginOperationAction::Disable
        };
        let transition = self.envelope.plan.packages.as_slice();
        let receipt = self.envelope.plan.state.receipt_digest.as_deref();
        let planned_state = transition
            .first()
            .and_then(|package| package.after.as_ref());
        if self.envelope.plan.operation_id != self.request.operation_id
            || self.envelope.plan.action != expected_plan_action
            || self.envelope.plan.package_id != state.package_id
            || self.envelope.plan.scope != state.scope
            || self.envelope.plan.state.state_revision == 0
            || transition.len() != 1
            || transition[0].package_id != state.package_id
            || transition[0].change != PlanPackageChangeKind::Retain
            || transition[0].before != transition[0].after
            || planned_state.is_none_or(|planned| {
                planned.release.version != artifact.version
                    || planned.release.package_sha256 != artifact.package_digest
                    || planned.release.manifest_sha256 != artifact.manifest_digest
            })
            || receipt.is_none()
            || self.intent.plan_digest != self.envelope.plan_digest
        {
            return Err(store_invalid(
                "A pending enablement plan drifted from its exact installed artifact or request.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
/// Snapshot-bound materialization and crash-recovery evidence.
///
/// This record never selects desired state. Its package state, installation
/// generation, and digest must be re-derived from `InstallationSnapshot`.
pub(super) struct StoredCognitivePackageEnablement {
    pub schema: String,
    pub scope: PlanScope,
    pub package_id: String,
    pub installation_generation: u64,
    pub installation_snapshot_digest: String,
    pub state_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact: Option<CognitivePackageArtifactState>,
    pub enabled: bool,
    pub updated_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<PendingCognitivePackageEnablement>,
}

impl StoredCognitivePackageEnablement {
    pub fn new(
        scope: PlanScope,
        package_id: impl Into<String>,
        installation_snapshot: &InstallationSnapshot,
        state_generation: u64,
        artifact: Option<CognitivePackageArtifactState>,
        enabled: bool,
        updated_at_ms: u64,
    ) -> UseResult<Self> {
        let state = Self {
            schema: ENABLEMENT_STATE_SCHEMA.to_string(),
            scope,
            package_id: package_id.into(),
            installation_generation: installation_snapshot.generation,
            installation_snapshot_digest: installation_snapshot.descriptor_digest()?,
            state_generation,
            artifact,
            enabled,
            updated_at_ms,
            active: None,
        };
        state.validate()?;
        Ok(state)
    }

    pub fn validate(&self) -> UseResult<()> {
        validate_scope(&self.scope)?;
        PluginPackageId::parse(self.package_id.clone()).map_err(|_| {
            store_invalid("A cognitive-package enablement package identity is invalid.")
        })?;
        if self.schema != ENABLEMENT_STATE_SCHEMA
            || self.installation_generation == 0
            || !valid_sha256(&self.installation_snapshot_digest)
            || self.state_generation == 0
            || self.updated_at_ms == 0
            || (self.artifact.is_none() && self.enabled)
        {
            return Err(store_invalid(
                "A cognitive-package enablement recovery projection is invalid.",
            ));
        }
        if let Some(artifact) = &self.artifact {
            artifact.validate()?;
        }
        if let Some(active) = &self.active {
            active.validate_against(self)?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StoredCognitivePackageEnablementOperation {
    pub schema: String,
    pub scope: PlanScope,
    pub request_digest: String,
    pub request: CognitivePackageEnablementRequest,
    pub envelope: PluginOperationPlanEnvelope,
    pub authorization: PackageGraphAuthorization,
    pub admitted_at_ms: u64,
    pub result: CognitivePackageEnablementResult,
    pub state_after: StoredCognitivePackageEnablement,
}

impl StoredCognitivePackageEnablementOperation {
    pub fn new(
        scope: PlanScope,
        request: CognitivePackageEnablementRequest,
        envelope: PluginOperationPlanEnvelope,
        authorization: PackageGraphAuthorization,
        admitted_at_ms: u64,
        result: CognitivePackageEnablementResult,
        state_after: StoredCognitivePackageEnablement,
    ) -> UseResult<Self> {
        let operation = Self {
            schema: ENABLEMENT_OPERATION_SCHEMA.to_string(),
            request_digest: request.descriptor_digest()?,
            scope,
            request,
            envelope,
            authorization,
            admitted_at_ms,
            result,
            state_after,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub fn validate(&self) -> UseResult<()> {
        validate_scope(&self.scope)?;
        self.request.validate()?;
        self.result.validate_for(&self.request)?;
        self.state_after.validate()?;
        self.envelope.validate()?;
        self.authorization
            .validate_against(&self.envelope, self.admitted_at_ms)?;
        let expected_action = if self.request.enabled {
            PluginOperationAction::Enable
        } else {
            PluginOperationAction::Disable
        };
        if self.schema != ENABLEMENT_OPERATION_SCHEMA
            || self.admitted_at_ms == 0
            || self.envelope.plan.operation_id != self.request.operation_id
            || self.envelope.plan.package_id != self.request.package_id.as_str()
            || self.envelope.plan.scope != self.scope
            || self.envelope.plan.action != expected_action
        {
            return Err(store_invalid(
                "A completed enablement plan does not bind its exact request and scope.",
            ));
        }
        let package_generation = self.result.state.package_generation.ok_or_else(|| {
            store_invalid("A stored enablement result omitted its state generation.")
        })?;
        let artifact = self.state_after.artifact.as_ref().ok_or_else(|| {
            store_invalid("A completed enablement operation omitted its artifact identity.")
        })?;
        if !valid_sha256(&self.request_digest)
            || self.request.descriptor_digest()? != self.request_digest
            || self.result.replayed
            || self.state_after.scope != self.scope
            || self.state_after.package_id != self.request.package_id.as_str()
            || self.state_after.state_generation != package_generation
            || self.state_after.enabled != self.request.enabled
            || self.state_after.active.is_some()
            || self.state_after.updated_at_ms != self.result.completed_at_ms
            || self.result.state.version.as_deref() != Some(artifact.version.as_str())
            || self.result.state.package_digest.as_deref() != Some(artifact.package_digest.as_str())
            || self.result.state.manifest_digest.as_deref()
                != Some(artifact.manifest_digest.as_str())
        {
            return Err(store_invalid(
                "A completed cognitive-package enablement operation is invalid.",
            ));
        }
        Ok(())
    }
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn validate_scope(scope: &PlanScope) -> UseResult<()> {
    scope.validate().map_err(|_| {
        store_invalid("A cognitive-package enablement installation identity is invalid.")
    })
}

pub(super) fn operation_conflict() -> UseError {
    UseError::new(
        "use.plugin.package_enablement_operation_conflict",
        "The enablement operation ID is already bound to a different durable request or result.",
    )
}

fn store_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_enablement_store_invalid", message)
}

