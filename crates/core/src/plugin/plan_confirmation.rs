use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::{valid_machine_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanActor,
    PlanPolicyDecision, PluginOperationPlan, PluginOperationPlanEnvelope,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA,
};

const CONFIRMATION_ERROR: &str = "use.plugin.plan_confirmation_invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationConfirmation {
    pub schema: String,
    pub operation_id: String,
    pub plan_digest: String,
    pub confirmed_by: PlanActor,
    pub confirmed_at_ms: u64,
}

impl PluginOperationConfirmation {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin operation confirmation",
            CONFIRMATION_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_OPERATION_CONFIRMATION_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.plan_digest)
            || self.confirmed_by != PlanActor::User
            || self.confirmed_at_ms == 0
        {
            return Err(confirmation_error(
                "The plugin operation confirmation identity, digest, actor, or time is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin operation confirmation", CONFIRMATION_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginOperationPlanEnvelope {
    /// Verifies the immutable plan and the confirmation required to apply it.
    pub fn verify_confirmed_apply(
        &self,
        operation_id: &str,
        plan_digest: &str,
        confirmation: Option<&PluginOperationConfirmation>,
        now_ms: u64,
    ) -> UseResult<()> {
        self.validate()?;
        verify_plan_confirmed_apply(
            &self.plan,
            &self.plan_digest,
            operation_id,
            plan_digest,
            confirmation,
            now_ms,
        )
    }
}

pub(super) fn verify_plan_confirmed_apply(
    plan: &PluginOperationPlan,
    expected_plan_digest: &str,
    operation_id: &str,
    plan_digest: &str,
    confirmation: Option<&PluginOperationConfirmation>,
    now_ms: u64,
) -> UseResult<()> {
    super::plan::verify_plan_apply(
        plan,
        expected_plan_digest,
        operation_id,
        plan_digest,
        now_ms,
    )?;
    match (plan.authority.decision, confirmation) {
        (PlanPolicyDecision::Allow, None) => Ok(()),
        (PlanPolicyDecision::Ask, None) => Err(UseError::new(
            "use.plugin.plan_confirmation_required",
            "The plugin operation plan requires explicit user confirmation.",
        )),
        (PlanPolicyDecision::Ask, Some(confirmation)) => {
            confirmation.validate()?;
            if confirmation.operation_id != plan.operation_id
                || confirmation.plan_digest != expected_plan_digest
                || confirmation.confirmed_at_ms < plan.created_at_ms
                || confirmation.confirmed_at_ms >= plan.expires_at_ms
                || confirmation.confirmed_at_ms > now_ms
            {
                return Err(confirmation_mismatch());
            }
            Ok(())
        }
        (PlanPolicyDecision::Allow, Some(_)) => Err(confirmation_mismatch()),
        (PlanPolicyDecision::Deny, _) => Err(UseError::new(
            "use.plugin.plan_denied",
            "Policy denies applying the plugin operation plan.",
        )),
    }
}

fn confirmation_error(message: impl Into<String>) -> UseError {
    contract_error(CONFIRMATION_ERROR, message)
}

fn confirmation_mismatch() -> UseError {
    UseError::new(
        "use.plugin.plan_confirmation_mismatch",
        "User confirmation does not bind the exact active plugin operation plan.",
    )
}
