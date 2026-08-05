use a3s_use_core::{
    PlanActor, PlanAuthority, PluginGrantConfirmation, PluginOperationConfirmation,
    PluginOperationPlan, PluginOperationPlanBinding, PluginOperationPlanDraft,
    PluginOperationPlanEnvelope, PluginWorkspaceGrantChangeSet, UseError, UseResult,
    PLUGIN_GRANT_CONFIRMATION_SCHEMA,
};
use async_trait::async_trait;

use super::grant::{CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider};

/// Exact authorization forwarded by a trusted umbrella or managed host.
///
/// This provider never creates a second operation identity. It requires the
/// package planner, dependency locks, provider evidence, Grant impacts, and
/// canonical plan digest to reproduce the host-reviewed envelope exactly,
/// then reuses only the confirmation bound to that envelope. Package content
/// cannot alter any host-owned field.
#[derive(Debug, Clone)]
pub struct ReviewedCognitivePackageAuthorizationProvider {
    expected: PluginOperationPlanEnvelope,
    confirmation: Option<PluginOperationConfirmation>,
}

impl ReviewedCognitivePackageAuthorizationProvider {
    pub fn new(
        expected: PluginOperationPlanEnvelope,
        confirmation: Option<PluginOperationConfirmation>,
    ) -> UseResult<Self> {
        expected.validate().map_err(|_| {
            reviewed_authorization_error(
                "The reviewed cognitive-package operation plan is invalid.",
            )
        })?;
        let validation_time = confirmation
            .as_ref()
            .map_or(expected.plan.created_at_ms, |value| value.confirmed_at_ms);
        expected
            .verify_confirmed_apply(
                &expected.plan.operation_id,
                &expected.plan_digest,
                confirmation.as_ref(),
                validation_time,
            )
            .map_err(|_| {
                reviewed_authorization_error(
                    "The forwarded confirmation does not authorize the exact reviewed cognitive-package plan.",
                )
            })?;
        Ok(Self {
            expected,
            confirmation,
        })
    }

    pub fn expected_plan(&self) -> &PluginOperationPlanEnvelope {
        &self.expected
    }

    fn verify_draft(&self, draft: &PluginOperationPlanDraft) -> UseResult<()> {
        draft.validate()?;
        let expected = &self.expected.plan;
        if !draft.workspace_impacts.is_empty()
            || draft.action != expected.action
            || draft.package_id != expected.package_id
            || draft.component_id != expected.component_id
            || draft.packages != expected.packages
            || draft.providers != expected.providers
            || draft.impact != expected.impact
            || draft.state != expected.state
        {
            return Err(reviewed_plan_mismatch(
                "The local cognitive-package planner no longer reproduces the reviewed host plan.",
            ));
        }
        Ok(())
    }
}

#[async_trait]
impl CognitivePackageAuthorizationProvider for ReviewedCognitivePackageAuthorizationProvider {
    fn name(&self) -> &'static str {
        "reviewed-host-plan"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        self.verify_draft(draft)?;
        Ok(self.expected.plan.authority.clone())
    }

    fn bind_operation(
        &self,
        draft: &PluginOperationPlanDraft,
        default_binding: PluginOperationPlanBinding,
    ) -> UseResult<PluginOperationPlanBinding> {
        self.verify_draft(draft)?;
        if default_binding.scope != self.expected.plan.scope {
            return Err(reviewed_plan_mismatch(
                "The reviewed host plan scope does not match the cognitive-package manager scope.",
            ));
        }
        Ok(PluginOperationPlanBinding {
            operation_id: self.expected.plan.operation_id.clone(),
            created_at_ms: self.expected.plan.created_at_ms,
            expires_at_ms: self.expected.plan.expires_at_ms,
            scope: self.expected.plan.scope.clone(),
            authority: self.expected.plan.authority.clone(),
        })
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        if plan != &self.expected.plan {
            return Err(reviewed_plan_mismatch(
                "The cognitive-package plan changed after host review.",
            ));
        }
        Ok(())
    }

    fn verify_plan(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        envelope.validate()?;
        if envelope != &self.expected {
            return Err(reviewed_plan_mismatch(
                "The cognitive-package plan or dependency-lock evidence changed after host review.",
            ));
        }
        Ok(())
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        self.verify_plan(envelope)?;
        envelope.verify_confirmed_apply(
            &envelope.plan.operation_id,
            &envelope.plan_digest,
            self.confirmation.as_ref(),
            now_ms,
        )?;
        let grant_confirmations = match &self.confirmation {
            Some(confirmation) => changes
                .into_iter()
                .flat_map(|changes| &changes.changes)
                .filter_map(|change| change.after.as_ref())
                .map(|proposal| {
                    Ok(PluginGrantConfirmation {
                        schema: PLUGIN_GRANT_CONFIRMATION_SCHEMA.to_string(),
                        operation_id: envelope.plan.operation_id.clone(),
                        plan_digest: envelope.plan_digest.clone(),
                        proposal_digest: proposal.descriptor_digest()?,
                        confirmed_by: PlanActor::User,
                        confirmed_at_ms: confirmation.confirmed_at_ms,
                    })
                })
                .collect::<UseResult<Vec<_>>>()?,
            None => Vec::new(),
        };
        Ok(CognitivePackageAuthorizationEvidence {
            operation_confirmation: self.confirmation.clone(),
            grant_confirmations,
        })
    }
}

fn reviewed_authorization_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_reviewed_authorization_invalid", message)
}

fn reviewed_plan_mismatch(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.package_reviewed_plan_mismatch", message)
}
