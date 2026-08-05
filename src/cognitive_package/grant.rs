use std::collections::BTreeSet;

use a3s_use_core::{
    PlanActor, PlanAuthority, PlanEnforcementProfile, PlanPackageChangeKind, PlanPolicyDecision,
    PlannedPackageState, PlannedWorkspaceGrantChange, PluginGrantConfirmation,
    PluginOperationConfirmation, PluginOperationPlan, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginWorkspaceGrantChangeSet,
    PluginWorkspaceGrantProposal, PluginWorkspaceGrantSnapshot, ResolvedWorkspaceGrantChangeSet,
    UseError, UseResult, WorkspaceGrantProposalAuthority, PLUGIN_GRANT_CONFIRMATION_SCHEMA,
    PLUGIN_OPERATION_CONFIRMATION_SCHEMA, PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA,
};
use a3s_use_extension::{WorkspaceGrantCandidateCeiling, WorkspaceGrantStore};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::PluginGrantLifecycleUnit;

use super::package_manager_error;

const STANDALONE_POLICY: &str = "a3s-use-standalone-cognitive-package-policy-v1";

/// Confirmation evidence returned by a trusted user- or host-facing adapter.
///
/// The provider receives the exact immutable package plan and canonical Grant
/// change set before creating this value. Package content cannot select the
/// actor, policy decision, confirmation time, or confirmed plan digest.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CognitivePackageAuthorizationEvidence {
    pub operation_confirmation: Option<PluginOperationConfirmation>,
    pub grant_confirmations: Vec<PluginGrantConfirmation>,
}

impl CognitivePackageAuthorizationEvidence {
    /// Create exact user confirmation for an `ask` plan and every candidate
    /// Grant proposal in the same reviewed event.
    pub fn confirmed(
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        confirmed_at_ms: u64,
    ) -> UseResult<Self> {
        envelope.validate()?;
        if envelope.plan.authority.actor != PlanActor::User
            || envelope.plan.authority.decision != PlanPolicyDecision::Ask
        {
            return Err(authorization_error(
                "Exact user confirmation is valid only for a user-owned `ask` plan.",
            ));
        }
        let operation_confirmation = PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms,
        };
        operation_confirmation.validate()?;
        let grant_confirmations = changes
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
                    confirmed_at_ms,
                })
            })
            .collect::<UseResult<Vec<_>>>()?;
        Ok(Self {
            operation_confirmation: Some(operation_confirmation),
            grant_confirmations,
        })
    }
}

/// Trusted authorization boundary for standalone cognitive-package mutation.
///
/// `bind_authority` runs after dependency, provider, permission, and impact
/// planning but before host authority is copied into the immutable plan.
/// `authorize` then receives the final plan and canonical Grant proposals.
/// Implementations must collect confirmation outside package-controlled code.
#[async_trait]
pub trait CognitivePackageAuthorizationProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority>;

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()>;

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        changes: Option<&PluginWorkspaceGrantChangeSet>,
        now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence>;
}

/// Fail-closed policy used when an embedding host does not inject its own
/// authorization provider.
///
/// Permission-free contributions and resource-bounded isolated execution can
/// proceed unattended. Ambient filesystem, network, secret, process, private
/// service, UI HTTP, or native-unconfined authority requires an exact user
/// confirmation supplied by a different trusted provider.
#[derive(Debug, Clone, Copy, Default)]
pub struct StandaloneCognitivePackageAuthorizationProvider;

#[async_trait]
impl CognitivePackageAuthorizationProvider for StandaloneCognitivePackageAuthorizationProvider {
    fn name(&self) -> &'static str {
        "standalone-fail-closed"
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        draft.validate()?;
        Ok(standalone_authority(
            draft
                .packages
                .iter()
                .flat_map(|package| [package.before.as_ref(), package.after.as_ref()])
                .flatten(),
            draft.providers.iter().map(|provider| provider.enforcement),
        ))
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        plan.validate()?;
        let expected = standalone_authority(
            plan.packages
                .iter()
                .flat_map(|package| [package.before.as_ref(), package.after.as_ref()])
                .flatten(),
            plan.providers.iter().map(|provider| provider.enforcement),
        );
        if plan.authority != expected {
            return Err(package_manager_error(
                "use.plugin.package_authority_changed",
                "The standalone cognitive-package authorization policy changed after planning.",
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
        self.verify_authority(&envelope.plan)?;
        match envelope.plan.authority.decision {
            PlanPolicyDecision::Allow => {
                envelope.verify_confirmed_apply(
                    &envelope.plan.operation_id,
                    &envelope.plan_digest,
                    None,
                    now_ms,
                )?;
                Ok(CognitivePackageAuthorizationEvidence::default())
            }
            PlanPolicyDecision::Ask => {
                let mut error = UseError::new(
                    "use.plugin.package_confirmation_required",
                    "The cognitive-package plan requests ambient authority and requires exact user confirmation.",
                )
                .with_detail("operationId", envelope.plan.operation_id.clone())
                .with_detail("planDigest", envelope.plan_digest.clone())
                .with_detail("plan", serde_json::to_value(envelope).unwrap_or_default())
                .with_suggestion(
                    "Review the immutable plan through a trusted host and apply it with an injected authorization provider.",
                );
                if let Some(changes) = changes {
                    error = error.with_detail(
                        "workspaceGrantChanges",
                        serde_json::to_value(changes).unwrap_or_default(),
                    );
                }
                Err(error)
            }
            PlanPolicyDecision::Deny => Err(UseError::new(
                "use.plugin.plan_denied",
                "Policy denies applying the cognitive-package operation plan.",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PlannedWorkspaceGrantOperation {
    pub snapshot: PluginWorkspaceGrantSnapshot,
    pub change_set: PluginWorkspaceGrantChangeSet,
    pub ceilings: Vec<WorkspaceGrantCandidateCeiling>,
}

/// Durable authorization evidence embedded in one pending package operation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct PackageGraphAuthorization {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_confirmation: Option<PluginOperationConfirmation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grant_confirmations: Vec<PluginGrantConfirmation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_snapshot: Option<PluginWorkspaceGrantSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_change_set: Option<PluginWorkspaceGrantChangeSet>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_grants: Option<ResolvedWorkspaceGrantChangeSet>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub grant_ceilings: Vec<WorkspaceGrantCandidateCeiling>,
}

impl PackageGraphAuthorization {
    pub fn validate_against(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        admitted_at_ms: u64,
    ) -> UseResult<()> {
        envelope.verify_confirmed_apply(
            &envelope.plan.operation_id,
            &envelope.plan_digest,
            self.operation_confirmation.as_ref(),
            admitted_at_ms,
        )?;
        for confirmation in &self.grant_confirmations {
            confirmation.validate()?;
        }

        let expected = expected_grant_packages(&envelope.plan);
        match (
            &self.grant_snapshot,
            &self.grant_change_set,
            &self.resolved_grants,
        ) {
            (None, None, None)
                if self.grant_ceilings.is_empty()
                    && self.grant_confirmations.is_empty()
                    && expected.is_empty()
                    && envelope.plan.workspace_impacts.is_empty() =>
            {
                Ok(())
            }
            (Some(snapshot), Some(change_set), Some(resolved)) if !expected.is_empty() => {
                validate_workspace_impact(&envelope.plan)?;
                snapshot.validate()?;
                if snapshot.scope_id != envelope.plan.scope.id
                    || snapshot.state_revision != envelope.plan.state.state_revision
                {
                    return Err(authorization_error(
                        "The persisted Grant snapshot does not bind the package plan scope and state revision.",
                    ));
                }
                change_set.validate_against_plan(&envelope.plan, Some(snapshot))?;
                let recomputed = change_set.finalize_against_plan(
                    &envelope.plan,
                    Some(snapshot),
                    self.operation_confirmation.as_ref(),
                    &self.grant_confirmations,
                    admitted_at_ms,
                )?;
                if &recomputed != resolved
                    || self.grant_ceilings != candidate_ceilings(&envelope.plan)?
                {
                    return Err(authorization_error(
                        "The persisted resolved Grants or signed ceilings drifted from the immutable package plan.",
                    ));
                }
                Ok(())
            }
            _ => Err(authorization_error(
                "A permission-bearing package operation omitted complete durable Grant authorization evidence.",
            )),
        }
    }

    pub fn lifecycle_unit(
        &self,
        store: WorkspaceGrantStore,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<Option<PluginGrantLifecycleUnit>> {
        let Some(resolved) = &self.resolved_grants else {
            return Ok(None);
        };
        PluginGrantLifecycleUnit::new(
            store,
            envelope.clone(),
            resolved.clone(),
            self.grant_ceilings.clone(),
        )
        .map(Some)
    }
}

pub(super) fn plan_workspace_grants(
    draft: &mut PluginOperationPlanDraft,
    binding: &PluginOperationPlanBinding,
    snapshot: &PluginWorkspaceGrantSnapshot,
    enabled_before: bool,
    enabled_after: bool,
) -> UseResult<Option<PlannedWorkspaceGrantOperation>> {
    snapshot.validate()?;
    if !draft.workspace_impacts.is_empty()
        || snapshot.scope_id != binding.scope.id
        || snapshot.state_revision != draft.state.state_revision
    {
        return Err(authorization_error(
            "The Grant snapshot does not bind the package draft scope and state revision.",
        ));
    }

    let mut changes = Vec::new();
    let mut ceilings = Vec::new();
    for package in &draft.packages {
        let before_required = enabled_before
            && matches!(
                package.change,
                PlanPackageChangeKind::Remove | PlanPackageChangeKind::Replace
            )
            && package
                .before
                .as_ref()
                .is_some_and(has_workspace_permissions);
        let after_required = enabled_after
            && matches!(
                package.change,
                PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
            )
            && package
                .after
                .as_ref()
                .is_some_and(has_workspace_permissions);
        if !before_required && !after_required {
            continue;
        }
        let before = if before_required {
            let before = package.before.as_ref().ok_or_else(|| {
                authorization_error("A Grant retirement omitted its prior package state.")
            })?;
            Some(
                snapshot
                    .grants
                    .iter()
                    .find(|evidence| {
                        evidence.package_id == package.package_id
                            && evidence.package_digest == before.release.package_sha256
                    })
                    .cloned()
                    .ok_or_else(|| {
                        package_manager_error(
                            "use.plugin.package_grant_reconciliation_required",
                            format!(
                                "Permission-bearing package '{}' has no exact active Grant to retire.",
                                package.package_id
                            ),
                        )
                    })?,
            )
        } else {
            None
        };
        let after = if after_required {
            let state = package.after.as_ref().ok_or_else(|| {
                authorization_error("A candidate Grant omitted its package state.")
            })?;
            ceilings.push(WorkspaceGrantCandidateCeiling {
                package_id: package.package_id.clone(),
                package_digest: state.release.package_sha256.clone(),
                ceiling: state.permissions.clone(),
            });
            Some(grant_proposal(binding, state)?)
        } else {
            None
        };
        changes.push(PlannedWorkspaceGrantChange {
            package_id: package.package_id.clone(),
            before,
            after,
        });
    }
    if changes.is_empty() {
        return Ok(None);
    }

    let before_snapshot_digest = snapshot.descriptor_digest()?;
    let change_set = PluginWorkspaceGrantChangeSet {
        schema: PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA.to_string(),
        operation_id: binding.operation_id.clone(),
        scope_id: binding.scope.id.clone(),
        state_revision: draft.state.state_revision,
        before_snapshot_digest: Some(before_snapshot_digest.clone()),
        changes,
    };
    change_set.validate()?;
    draft
        .workspace_impacts
        .push(a3s_use_core::PlannedWorkspaceImpact {
            scope_id: binding.scope.id.clone(),
            grant_before_digest: Some(before_snapshot_digest),
            grant_after_digest: Some(change_set.descriptor_digest()?),
            enabled_before,
            enabled_after,
        });
    draft.validate()?;
    Ok(Some(PlannedWorkspaceGrantOperation {
        snapshot: snapshot.clone(),
        change_set,
        ceilings,
    }))
}

pub(super) async fn authorize_planned_operation(
    provider: &dyn CognitivePackageAuthorizationProvider,
    envelope: &PluginOperationPlanEnvelope,
    planned: Option<&PlannedWorkspaceGrantOperation>,
    admitted_at_ms: u64,
) -> UseResult<PackageGraphAuthorization> {
    provider.verify_authority(&envelope.plan)?;
    let evidence = provider
        .authorize(
            envelope,
            planned.map(|planned| &planned.change_set),
            admitted_at_ms,
        )
        .await?;
    let resolved_grants = planned
        .map(|planned| {
            planned.change_set.finalize_against_plan(
                &envelope.plan,
                Some(&planned.snapshot),
                evidence.operation_confirmation.as_ref(),
                &evidence.grant_confirmations,
                admitted_at_ms,
            )
        })
        .transpose()?;
    let authorization = PackageGraphAuthorization {
        operation_confirmation: evidence.operation_confirmation,
        grant_confirmations: evidence.grant_confirmations,
        grant_snapshot: planned.map(|planned| planned.snapshot.clone()),
        grant_change_set: planned.map(|planned| planned.change_set.clone()),
        resolved_grants,
        grant_ceilings: planned
            .map(|planned| planned.ceilings.clone())
            .unwrap_or_default(),
    };
    authorization.validate_against(envelope, admitted_at_ms)?;
    Ok(authorization)
}

fn standalone_authority<'a>(
    states: impl IntoIterator<Item = &'a PlannedPackageState>,
    enforcement: impl IntoIterator<Item = PlanEnforcementProfile>,
) -> PlanAuthority {
    let ambient = states.into_iter().any(|state| {
        state.permissions.surfaces.iter().any(|permission| {
            permission.native_execution
                || permission.child_process
                || !permission.filesystem.is_empty()
                || !permission.network_egress.is_empty()
                || permission.private_service
                || !permission.secrets.is_empty()
                || !permission.ui_http.is_empty()
        })
    }) || enforcement
        .into_iter()
        .any(|profile| profile == PlanEnforcementProfile::NativeUnconfined);
    let decision = if ambient {
        PlanPolicyDecision::Ask
    } else {
        PlanPolicyDecision::Allow
    };
    PlanAuthority {
        actor: PlanActor::User,
        decision,
        policy_digest: format!("sha256:{:x}", Sha256::digest(STANDALONE_POLICY.as_bytes())),
        confirmation_required: decision == PlanPolicyDecision::Ask,
    }
}

fn grant_proposal(
    binding: &PluginOperationPlanBinding,
    state: &PlannedPackageState,
) -> UseResult<PluginWorkspaceGrantProposal> {
    let proposal = PluginWorkspaceGrantProposal {
        schema: PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA.to_string(),
        operation_id: binding.operation_id.clone(),
        scope_id: binding.scope.id.clone(),
        package_id: state.release.package_id.clone(),
        package_digest: state.release.package_sha256.clone(),
        permission_ceiling_digest: state.release.permission_ceiling_digest.clone(),
        permissions_digest: state.permissions.descriptor_digest()?,
        permissions: state.permissions.clone(),
        authority: WorkspaceGrantProposalAuthority {
            actor: binding.authority.actor,
            decision: binding.authority.decision,
            policy_digest: binding.authority.policy_digest.clone(),
        },
        created_at_ms: binding.created_at_ms,
        apply_expires_at_ms: binding.expires_at_ms,
        grant_expires_at_ms: None,
    };
    proposal.validate_against(&state.permissions)?;
    Ok(proposal)
}

fn has_workspace_permissions(state: &PlannedPackageState) -> bool {
    !state.permissions.surfaces.is_empty()
}

fn expected_grant_packages(plan: &PluginOperationPlan) -> BTreeSet<&str> {
    let (enabled_before, enabled_after) = expected_enablement(plan);
    plan.packages
        .iter()
        .filter(|package| {
            (enabled_before
                && matches!(
                    package.change,
                    PlanPackageChangeKind::Remove | PlanPackageChangeKind::Replace
                )
                && package
                    .before
                    .as_ref()
                    .is_some_and(has_workspace_permissions))
                || (enabled_after
                    && matches!(
                        package.change,
                        PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
                    )
                    && package
                        .after
                        .as_ref()
                        .is_some_and(has_workspace_permissions))
        })
        .map(|package| package.package_id.as_str())
        .collect()
}

fn candidate_ceilings(
    plan: &PluginOperationPlan,
) -> UseResult<Vec<WorkspaceGrantCandidateCeiling>> {
    let (_, enabled_after) = expected_enablement(plan);
    let ceilings = plan
        .packages
        .iter()
        .filter(|package| {
            enabled_after
                && matches!(
                    package.change,
                    PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
                )
                && package
                    .after
                    .as_ref()
                    .is_some_and(has_workspace_permissions)
        })
        .map(|package| {
            let state = package.after.as_ref().ok_or_else(|| {
                authorization_error("A candidate Grant ceiling omitted its package state.")
            })?;
            Ok(WorkspaceGrantCandidateCeiling {
                package_id: package.package_id.clone(),
                package_digest: state.release.package_sha256.clone(),
                ceiling: state.permissions.clone(),
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    for ceiling in &ceilings {
        ceiling.validate()?;
    }
    Ok(ceilings)
}

fn validate_workspace_impact(plan: &PluginOperationPlan) -> UseResult<()> {
    let (enabled_before, enabled_after) = expected_enablement(plan);
    if plan.workspace_impacts.len() != 1
        || plan.workspace_impacts[0].scope_id != plan.scope.id
        || plan.workspace_impacts[0].enabled_before != enabled_before
        || plan.workspace_impacts[0].enabled_after != enabled_after
        || plan.workspace_impacts[0].grant_before_digest.is_none()
        || plan.workspace_impacts[0].grant_after_digest.is_none()
    {
        return Err(authorization_error(
            "The package plan does not bind the exact Grant scope and enablement transition.",
        ));
    }
    Ok(())
}

fn expected_enablement(plan: &PluginOperationPlan) -> (bool, bool) {
    match plan.action {
        a3s_use_core::PluginOperationAction::Install => (false, true),
        a3s_use_core::PluginOperationAction::Upgrade => (true, true),
        a3s_use_core::PluginOperationAction::Uninstall => (true, false),
    }
}

fn authorization_error(message: impl Into<String>) -> UseError {
    package_manager_error("use.plugin.package_authorization_invalid", message)
}

#[cfg(test)]
#[path = "grant_tests.rs"]
mod tests;
