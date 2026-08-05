use serde::{Deserialize, Serialize};

use crate::UseResult;

use super::plan::{
    PlanAuthority, PlanScope, PlannedOperationImpact, PlannedPackageTransition,
    PlannedProviderEvidence, PlannedStateEvidence, PlannedWorkspaceImpact, PluginOperationAction,
    PluginOperationPlan,
};
use super::plan_validation::{planned_okf_changes, planned_secret_changes};
use super::{
    parse_contract, plan::plan_error, PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA,
    PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2, PLUGIN_OPERATION_PLAN_SCHEMA,
    PLUGIN_OPERATION_PLAN_SCHEMA_V2,
};

/// Planner-owned operation evidence before the host assigns identity, scope,
/// principal, lifetime, policy, or confirmation requirements.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPlanDraft {
    pub schema: String,
    pub action: PluginOperationAction,
    pub package_id: String,
    pub component_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package_lock_digest: Option<String>,
    pub packages: Vec<PlannedPackageTransition>,
    pub providers: Vec<PlannedProviderEvidence>,
    pub workspace_impacts: Vec<PlannedWorkspaceImpact>,
    pub impact: PlannedOperationImpact,
    pub state: PlannedStateEvidence,
}

/// Host-owned fields that turn untrusted planner evidence into an immutable
/// operation plan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginOperationPlanBinding {
    pub operation_id: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub scope: PlanScope,
    pub authority: PlanAuthority,
}

impl PluginOperationPlanDraft {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        action: PluginOperationAction,
        package_id: impl Into<String>,
        component_id: impl Into<String>,
        packages: Vec<PlannedPackageTransition>,
        providers: Vec<PlannedProviderEvidence>,
        workspace_impacts: Vec<PlannedWorkspaceImpact>,
        impact: PlannedOperationImpact,
        state: PlannedStateEvidence,
    ) -> UseResult<Self> {
        let mut impact = impact;
        impact.okf_changes = planned_okf_changes(&packages)?;
        let schema = if impact.okf_changes.is_empty() {
            PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA
        } else {
            PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2
        };
        let draft = Self {
            schema: schema.to_string(),
            action,
            package_id: package_id.into(),
            component_id: component_id.into(),
            package_lock_digest: None,
            packages,
            providers,
            workspace_impacts,
            impact,
            state,
        };
        draft.validate()?;
        Ok(draft)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin operation plan draft",
            super::plan::PLAN_ERROR,
            Self::validate,
        )
    }

    /// Validate planner-owned evidence without granting it a real host
    /// identity or authority.
    pub fn validate(&self) -> UseResult<()> {
        if !matches!(
            self.schema.as_str(),
            PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA | PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2
        ) {
            return Err(plan_error(
                "The plugin operation plan draft schema is unsupported.",
            ));
        }
        self.clone().bind_unchecked(validation_binding()).map(drop)
    }

    pub fn bind(self, binding: PluginOperationPlanBinding) -> UseResult<PluginOperationPlan> {
        if !matches!(
            self.schema.as_str(),
            PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA | PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2
        ) {
            return Err(plan_error(
                "The plugin operation plan draft schema is unsupported.",
            ));
        }
        self.bind_unchecked(binding)
    }

    fn bind_unchecked(self, binding: PluginOperationPlanBinding) -> UseResult<PluginOperationPlan> {
        let secret_changes = planned_secret_changes(&self.packages);
        let plan_schema = match self.schema.as_str() {
            PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA => PLUGIN_OPERATION_PLAN_SCHEMA,
            PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V2 => PLUGIN_OPERATION_PLAN_SCHEMA_V2,
            _ => {
                return Err(plan_error(
                    "The plugin operation plan draft schema is unsupported.",
                ))
            }
        };
        let plan = PluginOperationPlan {
            schema: plan_schema.to_string(),
            operation_id: binding.operation_id,
            created_at_ms: binding.created_at_ms,
            expires_at_ms: binding.expires_at_ms,
            action: self.action,
            package_id: self.package_id,
            component_id: self.component_id,
            scope: binding.scope,
            package_lock_digest: self.package_lock_digest,
            prior_package_lock_digest: None,
            packages: self.packages,
            secret_changes,
            providers: self.providers,
            workspace_impacts: self.workspace_impacts,
            impact: self.impact,
            authority: binding.authority,
            state: self.state,
        };
        plan.validate()?;
        Ok(plan)
    }
}

fn validation_binding() -> PluginOperationPlanBinding {
    PluginOperationPlanBinding {
        operation_id: "draft:validation".to_string(),
        created_at_ms: 1,
        expires_at_ms: 2,
        scope: PlanScope {
            kind: super::PlanScopeKind::User,
            id: "current".to_string(),
        },
        authority: PlanAuthority {
            actor: super::PlanActor::User,
            decision: super::PlanPolicyDecision::Ask,
            policy_digest:
                "sha256:0000000000000000000000000000000000000000000000000000000000000000"
                    .to_string(),
            confirmation_required: true,
        },
    }
}
