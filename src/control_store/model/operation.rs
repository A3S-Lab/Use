use a3s_use_core::{
    InstallationId, InstallationSnapshot, PlanAuthority, PlanPolicyDecision,
    PluginGrantConfirmation, PluginOperationAction, PluginOperationConfirmation,
    PluginOperationPlanEnvelope, PluginWorkspaceGrantChangeSet, PluginWorkspaceGrantSnapshot,
    UseResult,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{generation_exhausted, input_error, MAX_CONTROL_GRANTS};

pub(in crate::control_store) const MAX_CONTROL_OPERATION_PLAN_BYTES: usize = 16 * 1024 * 1024;
pub(in crate::control_store) const MAX_CONTROL_AUTHORIZATION_BYTES: usize = 4 * 1024 * 1024;
const CONTROL_AUTHORIZATION_EVIDENCE_SCHEMA: &str = "a3s.use.control-authorization-evidence.v2";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlGrantAuthorizationEvidence {
    pub(in crate::control_store) snapshot: PluginWorkspaceGrantSnapshot,
    pub(in crate::control_store) change_set: PluginWorkspaceGrantChangeSet,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlAuthorizationEvidence {
    schema: String,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) authority: PlanAuthority,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::control_store) operation_confirmation: Option<PluginOperationConfirmation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(in crate::control_store) grant_transition: Option<ControlGrantAuthorizationEvidence>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(in crate::control_store) grant_confirmations: Vec<PluginGrantConfirmation>,
}

impl ControlAuthorizationEvidence {
    pub(in crate::control_store) fn new(
        envelope: &PluginOperationPlanEnvelope,
        operation_confirmation: Option<PluginOperationConfirmation>,
        grant_transition: Option<ControlGrantAuthorizationEvidence>,
        mut grant_confirmations: Vec<PluginGrantConfirmation>,
        reviewed_at_ms: u64,
    ) -> UseResult<Self> {
        grant_confirmations.sort_by(|left, right| left.proposal_digest.cmp(&right.proposal_digest));
        let evidence = Self {
            schema: CONTROL_AUTHORIZATION_EVIDENCE_SCHEMA.to_string(),
            plan_digest: envelope.plan_digest.clone(),
            authority: envelope.plan.authority.clone(),
            operation_confirmation,
            grant_transition,
            grant_confirmations,
        };
        evidence.validate(envelope, reviewed_at_ms)?;
        Ok(evidence)
    }

    pub(in crate::control_store) fn validate(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        reviewed_at_ms: u64,
    ) -> UseResult<()> {
        envelope
            .validate()
            .map_err(|_| invalid_reviewed_operation())?;
        if self.schema != CONTROL_AUTHORIZATION_EVIDENCE_SCHEMA
            || reviewed_at_ms == 0
            || self.plan_digest != envelope.plan_digest
            || self.authority != envelope.plan.authority
            || self.grant_confirmations.len() > MAX_CONTROL_GRANTS
            || self
                .grant_confirmations
                .windows(2)
                .any(|pair| pair[0].proposal_digest >= pair[1].proposal_digest)
        {
            return Err(invalid_reviewed_operation());
        }
        envelope
            .verify_confirmed_apply(
                &envelope.plan.operation_id,
                &envelope.plan_digest,
                self.operation_confirmation.as_ref(),
                reviewed_at_ms,
            )
            .map_err(|_| invalid_reviewed_operation())?;
        let operation_confirmation_time = self
            .operation_confirmation
            .as_ref()
            .map(|confirmation| confirmation.confirmed_at_ms);
        if self.grant_confirmations.iter().any(|confirmation| {
            confirmation.validate().is_err()
                || confirmation.operation_id != envelope.plan.operation_id
                || confirmation.plan_digest != envelope.plan_digest
                || confirmation.confirmed_at_ms < envelope.plan.created_at_ms
                || confirmation.confirmed_at_ms >= envelope.plan.expires_at_ms
                || confirmation.confirmed_at_ms > reviewed_at_ms
                || (self.authority.decision == PlanPolicyDecision::Ask
                    && operation_confirmation_time != Some(confirmation.confirmed_at_ms))
        }) || (self.authority.decision == PlanPolicyDecision::Allow
            && !self.grant_confirmations.is_empty())
        {
            return Err(invalid_reviewed_operation());
        }

        let grant_changes_required = envelope
            .plan
            .workspace_grant_changes_required()
            .map_err(|_| invalid_reviewed_operation())?;
        match (&self.grant_transition, grant_changes_required) {
            (None, false) if self.grant_confirmations.is_empty() => {}
            (Some(grant_transition), true) => {
                grant_transition
                    .snapshot
                    .validate()
                    .map_err(|_| invalid_reviewed_operation())?;
                if grant_transition.snapshot.scope_id != envelope.plan.scope.id
                    || grant_transition.snapshot.state_revision
                        != envelope.plan.state.state_revision
                {
                    return Err(invalid_reviewed_operation());
                }
                grant_transition
                    .change_set
                    .finalize_against_plan(
                        &envelope.plan,
                        Some(&grant_transition.snapshot),
                        self.operation_confirmation.as_ref(),
                        &self.grant_confirmations,
                        reviewed_at_ms,
                    )
                    .map(drop)
                    .map_err(|_| invalid_reviewed_operation())?;
            }
            _ => return Err(invalid_reviewed_operation()),
        }
        self.canonical_bytes().map(drop)
    }

    pub(in crate::control_store) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        canonical_bytes(
            self,
            MAX_CONTROL_AUTHORIZATION_BYTES,
            "authorization evidence",
        )
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ReviewedControlOperation {
    pub(in crate::control_store) envelope: PluginOperationPlanEnvelope,
    pub(in crate::control_store) authorization: ControlAuthorizationEvidence,
    pub(in crate::control_store) expected_generation: u64,
    pub(in crate::control_store) expected_capability_generation: u64,
    pub(in crate::control_store) reviewed_at_ms: u64,
}

impl ReviewedControlOperation {
    pub(in crate::control_store) fn new(
        envelope: PluginOperationPlanEnvelope,
        operation_confirmation: Option<PluginOperationConfirmation>,
        grant_transition: Option<ControlGrantAuthorizationEvidence>,
        grant_confirmations: Vec<PluginGrantConfirmation>,
        expected_generation: u64,
        expected_capability_generation: u64,
        reviewed_at_ms: u64,
    ) -> UseResult<Self> {
        let authorization = ControlAuthorizationEvidence::new(
            &envelope,
            operation_confirmation,
            grant_transition,
            grant_confirmations,
            reviewed_at_ms,
        )?;
        let operation = Self {
            envelope,
            authorization,
            expected_generation,
            expected_capability_generation,
            reviewed_at_ms,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub(in crate::control_store) fn operation_id(&self) -> &str {
        &self.envelope.plan.operation_id
    }

    pub(in crate::control_store) fn plan_digest(&self) -> &str {
        &self.envelope.plan_digest
    }

    pub(in crate::control_store) fn authorization_digest(&self) -> UseResult<String> {
        self.authorization.descriptor_digest()
    }

    pub(in crate::control_store) const fn action(&self) -> PluginOperationAction {
        self.envelope.plan.action
    }

    pub(in crate::control_store) fn root_package_id(&self) -> &str {
        &self.envelope.plan.package_id
    }

    pub(in crate::control_store) fn canonical_plan_bytes(&self) -> UseResult<Vec<u8>> {
        canonical_bytes(
            &self.envelope,
            MAX_CONTROL_OPERATION_PLAN_BYTES,
            "operation plan envelope",
        )
    }

    pub(in crate::control_store) fn target_generation(&self) -> UseResult<u64> {
        self.expected_generation
            .checked_add(1)
            .ok_or_else(generation_exhausted)
    }

    pub(in crate::control_store) fn target_capability_generation(&self) -> UseResult<u64> {
        self.expected_capability_generation
            .checked_add(1)
            .ok_or_else(generation_exhausted)
    }

    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.envelope
            .validate()
            .map_err(|_| invalid_reviewed_operation())?;
        if self.reviewed_at_ms == 0
            || self.envelope.plan.state.state_revision != self.target_generation()?
            || self.envelope.plan.state.capability_generation != self.expected_capability_generation
        {
            return Err(invalid_reviewed_operation());
        }
        self.authorization
            .validate(&self.envelope, self.reviewed_at_ms)?;
        self.canonical_plan_bytes().map(drop)
    }

    pub(in crate::control_store) fn validate_for_installation(
        &self,
        installation: &InstallationId,
    ) -> UseResult<()> {
        self.validate()?;
        if self.envelope.plan.scope != *installation {
            return Err(invalid_reviewed_operation());
        }
        Ok(())
    }

    pub(in crate::control_store) fn validate_snapshot_transition(
        &self,
        prior: Option<&InstallationSnapshot>,
        target: &InstallationSnapshot,
    ) -> UseResult<()> {
        self.validate_for_installation(&target.installation)?;
        let prior_matches = match (self.expected_generation, prior) {
            (0, None) => true,
            (generation, Some(snapshot)) => {
                generation > 0
                    && snapshot.generation == generation
                    && snapshot.installation == target.installation
            }
            _ => false,
        };
        if !prior_matches || target.generation != self.target_generation()? {
            return Err(input_error(
                "The Control Store action does not bind consecutive installation snapshots.",
            ));
        }

        let root_package_id = self.root_package_id();
        let before_is_root = prior.is_some_and(|snapshot| {
            snapshot
                .roots
                .binary_search_by(|root| root.package_id.as_str().cmp(root_package_id))
                .is_ok()
        });
        let after_is_root = target
            .roots
            .binary_search_by(|root| root.package_id.as_str().cmp(root_package_id))
            .is_ok();
        let before_enabled = prior.and_then(|snapshot| package_enabled(snapshot, root_package_id));
        let after_enabled = package_enabled(target, root_package_id);
        let action_matches = match self.action() {
            PluginOperationAction::Install => !before_is_root && after_is_root,
            PluginOperationAction::Upgrade => before_is_root && after_is_root,
            PluginOperationAction::Enable => {
                before_is_root
                    && after_is_root
                    && before_enabled == Some(false)
                    && after_enabled == Some(true)
            }
            PluginOperationAction::Disable => {
                before_is_root
                    && after_is_root
                    && before_enabled == Some(true)
                    && after_enabled == Some(false)
            }
            PluginOperationAction::Uninstall => before_is_root && !after_is_root,
        };
        if !action_matches {
            return Err(input_error(
                "The reviewed Control Store action contradicts the root package state transition.",
            ));
        }
        Ok(())
    }
}

fn canonical_bytes<T: Serialize>(value: &T, max_bytes: usize, label: &str) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        input_error(format!(
            "Failed to encode canonical Control Store {label}: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() > max_bytes {
        return Err(input_error(format!(
            "The canonical Control Store {label} exceeds its size bound."
        )));
    }
    Ok(bytes)
}

fn package_enabled(snapshot: &InstallationSnapshot, package_id: &str) -> Option<bool> {
    snapshot
        .packages
        .binary_search_by(|package| package.package_id().cmp(package_id))
        .ok()
        .map(|index| snapshot.packages[index].enabled)
}

fn invalid_reviewed_operation() -> a3s_use_core::UseError {
    input_error("The reviewed Control Store operation or authorization evidence is invalid.")
}
