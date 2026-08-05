use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::resolved_grant_changes::{ResolvedWorkspaceGrant, ResolvedWorkspaceGrantChangeSet};
use super::validation::{valid_machine_id, valid_package_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanPackageChangeKind,
    PlanPolicyDecision, PlannedPackageState, PlannedPackageTransition, PluginGrantConfirmation,
    PluginOperationConfirmation, PluginOperationPlan, PluginWorkspaceGrantProposal,
    WorkspaceGrantAuthority, MAX_PLUGIN_PLAN_ITEMS, PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA,
};

const SNAPSHOT_ERROR: &str = "use.plugin.grant_snapshot_invalid";
const CHANGE_SET_ERROR: &str = "use.plugin.grant_changes_invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginWorkspaceGrantSnapshot {
    pub schema: String,
    pub scope_id: String,
    pub state_revision: u64,
    pub grants: Vec<WorkspaceGrantEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantEvidence {
    pub package_id: String,
    pub package_digest: String,
    pub receipt_revision: u64,
    pub grant_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginWorkspaceGrantChangeSet {
    pub schema: String,
    pub operation_id: String,
    pub scope_id: String,
    pub state_revision: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_snapshot_digest: Option<String>,
    pub changes: Vec<PlannedWorkspaceGrantChange>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedWorkspaceGrantChange {
    pub package_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<WorkspaceGrantEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<PluginWorkspaceGrantProposal>,
}

impl PluginWorkspaceGrantSnapshot {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin workspace grant snapshot",
            SNAPSHOT_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA
            || !valid_machine_id(&self.scope_id)
            || self.state_revision == 0
            || self.grants.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .grants
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
        {
            return Err(snapshot_error(
                "The workspace grant snapshot identity, revision, or ordering is invalid.",
            ));
        }
        for evidence in &self.grants {
            evidence.validate().map_err(|_| {
                snapshot_error("The workspace grant snapshot contains invalid evidence.")
            })?;
            if evidence.receipt_revision > self.state_revision {
                return Err(snapshot_error(
                    "Workspace grant evidence cannot exceed the durable state revision.",
                ));
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin workspace grant snapshot", SNAPSHOT_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl WorkspaceGrantEvidence {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_package_id(&self.package_id)
            || !valid_sha256(&self.package_digest)
            || self.receipt_revision == 0
            || !valid_sha256(&self.grant_digest)
        {
            return Err(snapshot_error(
                "Workspace grant revision evidence is invalid.",
            ));
        }
        Ok(())
    }
}

impl PluginWorkspaceGrantChangeSet {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin workspace grant change set",
            CHANGE_SET_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_machine_id(&self.scope_id)
            || self.state_revision == 0
            || self.changes.is_empty()
            || self.changes.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .before_snapshot_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || self
                .changes
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
        {
            return Err(change_set_error(
                "The workspace grant change-set identity, revision, or ordering is invalid.",
            ));
        }
        let mut has_before = false;
        for change in &self.changes {
            change.validate(&self.operation_id, &self.scope_id)?;
            has_before |= change.before.is_some();
        }
        if has_before && self.before_snapshot_digest.is_none() {
            return Err(change_set_error(
                "A grant change set with prior evidence must bind a before snapshot.",
            ));
        }
        Ok(())
    }

    pub fn validate_against_plan(
        &self,
        plan: &PluginOperationPlan,
        before: Option<&PluginWorkspaceGrantSnapshot>,
    ) -> UseResult<()> {
        self.validate()?;
        plan.validate()?;
        let impact = plan
            .workspace_impacts
            .iter()
            .find(|impact| impact.scope_id == self.scope_id)
            .ok_or_else(plan_mismatch)?;
        if self.operation_id != plan.operation_id
            || self.state_revision != plan.state.state_revision
            || impact.grant_before_digest != self.before_snapshot_digest
            || impact.grant_after_digest.as_deref() != Some(&self.descriptor_digest()?)
        {
            return Err(plan_mismatch());
        }
        self.validate_snapshot(before)?;

        let packages = plan
            .packages
            .iter()
            .map(|package| (package.package_id.as_str(), package))
            .collect::<BTreeMap<_, _>>();
        let changes = self
            .changes
            .iter()
            .map(|change| (change.package_id.as_str(), change))
            .collect::<BTreeMap<_, _>>();
        let expected = expected_changes(plan, impact.enabled_before, impact.enabled_after);
        if changes.keys().copied().collect::<BTreeSet<_>>() != expected {
            return Err(plan_mismatch());
        }
        for (package_id, change) in changes {
            let package = packages.get(package_id).ok_or_else(plan_mismatch)?;
            self.validate_change_against_plan(
                plan,
                package,
                change,
                before,
                impact.enabled_before,
                impact.enabled_after,
            )?;
        }
        Ok(())
    }

    pub fn finalize_against_plan(
        &self,
        plan: &PluginOperationPlan,
        before: Option<&PluginWorkspaceGrantSnapshot>,
        operation_confirmation: Option<&PluginOperationConfirmation>,
        confirmations: &[PluginGrantConfirmation],
        applied_at_ms: u64,
    ) -> UseResult<ResolvedWorkspaceGrantChangeSet> {
        self.validate_against_plan(plan, before)?;
        let plan_digest = plan.descriptor_digest()?;
        super::plan_confirmation::verify_plan_confirmed_apply(
            plan,
            &plan_digest,
            &plan.operation_id,
            &plan_digest,
            operation_confirmation,
            applied_at_ms,
        )?;
        let operation_confirmation_digest = operation_confirmation
            .map(PluginOperationConfirmation::descriptor_digest)
            .transpose()?;
        let revocation_authority = WorkspaceGrantAuthority {
            actor: plan.authority.actor,
            decision: plan.authority.decision,
            policy_digest: plan.authority.policy_digest.clone(),
            confirmation_digest: operation_confirmation_digest,
        };
        revocation_authority.validate()?;
        let mut confirmation_map = BTreeMap::new();
        for confirmation in confirmations {
            confirmation.validate()?;
            if confirmation_map
                .insert(confirmation.proposal_digest.as_str(), confirmation)
                .is_some()
            {
                return Err(confirmation_mismatch());
            }
        }

        let revision = plan.state.state_revision.checked_add(1).ok_or_else(|| {
            UseError::new(
                "use.plugin.grant_changes_revision_exhausted",
                "The workspace grant state revision cannot advance.",
            )
        })?;
        let capability_generation_after = plan
            .state
            .capability_generation
            .checked_add(1)
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.grant_changes_generation_exhausted",
                    "The plugin capability generation cannot advance.",
                )
            })?;
        let mut grants = Vec::new();
        let mut revocations = Vec::new();
        for change in &self.changes {
            if let Some(proposal) = &change.after {
                let proposal_digest = proposal.descriptor_digest()?;
                let confirmation = confirmation_map.remove(proposal_digest.as_str());
                if plan.authority.decision == PlanPolicyDecision::Ask {
                    if let (Some(proposal_confirmation), Some(operation_confirmation)) =
                        (confirmation, operation_confirmation)
                    {
                        if proposal_confirmation.confirmed_at_ms
                            != operation_confirmation.confirmed_at_ms
                        {
                            return Err(confirmation_mismatch());
                        }
                    }
                }
                let grant = proposal.finalize(
                    &package_after(plan, &change.package_id)?.permissions,
                    &plan_digest,
                    confirmation,
                    applied_at_ms,
                )?;
                grants.push(ResolvedWorkspaceGrant {
                    proposal_digest,
                    grant,
                });
            }
            if let Some(evidence) = &change.before {
                revocations.push(evidence.clone());
            }
        }
        if !confirmation_map.is_empty() {
            return Err(confirmation_mismatch());
        }
        let resolved = ResolvedWorkspaceGrantChangeSet {
            operation_id: plan.operation_id.clone(),
            plan_digest,
            change_set_digest: self.descriptor_digest()?,
            scope_id: self.scope_id.clone(),
            state_revision_before: plan.state.state_revision,
            revision,
            capability_generation_before: plan.state.capability_generation,
            capability_generation_after,
            before_snapshot_digest: self.before_snapshot_digest.clone(),
            transitioned_at_ms: applied_at_ms,
            revocation_authority,
            grants,
            revocations,
        };
        resolved.validate()?;
        Ok(resolved)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin workspace grant change set", CHANGE_SET_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    fn validate_snapshot(&self, before: Option<&PluginWorkspaceGrantSnapshot>) -> UseResult<()> {
        match (&self.before_snapshot_digest, before) {
            (None, None) => Ok(()),
            (Some(expected), Some(snapshot)) => {
                snapshot.validate()?;
                if snapshot.scope_id != self.scope_id
                    || snapshot.state_revision != self.state_revision
                    || snapshot.descriptor_digest()? != *expected
                {
                    return Err(plan_mismatch());
                }
                Ok(())
            }
            _ => Err(plan_mismatch()),
        }
    }

    fn validate_change_against_plan(
        &self,
        plan: &PluginOperationPlan,
        package: &PlannedPackageTransition,
        change: &PlannedWorkspaceGrantChange,
        snapshot: Option<&PluginWorkspaceGrantSnapshot>,
        enabled_before: bool,
        enabled_after: bool,
    ) -> UseResult<()> {
        let before_required = grant_before_required(package, enabled_before);
        let after_required = grant_after_required(package, enabled_after);
        if change.before.is_some() != before_required || change.after.is_some() != after_required {
            return Err(plan_mismatch());
        }
        if let Some(evidence) = &change.before {
            let state = package.before.as_ref().ok_or_else(plan_mismatch)?;
            if evidence.package_digest != state.release.package_sha256
                || !snapshot.is_some_and(|snapshot| snapshot.grants.contains(evidence))
            {
                return Err(plan_mismatch());
            }
        }
        if let Some(proposal) = &change.after {
            let state = package.after.as_ref().ok_or_else(plan_mismatch)?;
            if proposal.package_digest != state.release.package_sha256
                || proposal.permission_ceiling_digest != state.release.permission_ceiling_digest
                || proposal.authority.actor != plan.authority.actor
                || proposal.authority.decision != plan.authority.decision
                || proposal.authority.policy_digest != plan.authority.policy_digest
                || proposal.created_at_ms != plan.created_at_ms
                || proposal.apply_expires_at_ms != plan.expires_at_ms
            {
                return Err(plan_mismatch());
            }
            proposal
                .validate_against(&state.permissions)
                .map_err(|_| plan_mismatch())?;
        }
        Ok(())
    }
}

impl PlannedWorkspaceGrantChange {
    fn validate(&self, operation_id: &str, scope_id: &str) -> UseResult<()> {
        if !valid_package_id(&self.package_id)
            || (self.before.is_none() && self.after.is_none())
            || self.before.as_ref().is_some_and(|evidence| {
                evidence.package_id != self.package_id || evidence.validate().is_err()
            })
            || self.after.as_ref().is_some_and(|proposal| {
                proposal.operation_id != operation_id
                    || proposal.scope_id != scope_id
                    || proposal.package_id != self.package_id
                    || proposal.validate().is_err()
            })
        {
            return Err(change_set_error(
                "A planned workspace grant change is invalid.",
            ));
        }
        Ok(())
    }
}

fn expected_changes(
    plan: &PluginOperationPlan,
    enabled_before: bool,
    enabled_after: bool,
) -> BTreeSet<&str> {
    plan.packages
        .iter()
        .filter(|package| {
            grant_before_required(package, enabled_before)
                || grant_after_required(package, enabled_after)
        })
        .map(|package| package.package_id.as_str())
        .collect()
}

fn grant_before_required(package: &PlannedPackageTransition, enabled: bool) -> bool {
    enabled
        && matches!(
            package.change,
            PlanPackageChangeKind::Remove | PlanPackageChangeKind::Replace
        )
        && package.before.as_ref().is_some_and(has_grant_permissions)
}

fn grant_after_required(package: &PlannedPackageTransition, enabled: bool) -> bool {
    enabled
        && matches!(
            package.change,
            PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
        )
        && package.after.as_ref().is_some_and(has_grant_permissions)
}

fn has_grant_permissions(state: &PlannedPackageState) -> bool {
    !state.permissions.surfaces.is_empty()
}

fn package_after<'a>(
    plan: &'a PluginOperationPlan,
    package_id: &str,
) -> UseResult<&'a PlannedPackageState> {
    plan.packages
        .iter()
        .find(|package| package.package_id == package_id)
        .and_then(|package| package.after.as_ref())
        .ok_or_else(plan_mismatch)
}

fn snapshot_error(message: impl Into<String>) -> UseError {
    contract_error(SNAPSHOT_ERROR, message)
}

pub(super) fn change_set_error(message: impl Into<String>) -> UseError {
    contract_error(CHANGE_SET_ERROR, message)
}

fn plan_mismatch() -> UseError {
    UseError::new(
        "use.plugin.grant_changes_plan_mismatch",
        "The workspace grant changes do not match the immutable operation plan and state.",
    )
}

fn confirmation_mismatch() -> UseError {
    UseError::new(
        "use.plugin.grant_changes_confirmation_mismatch",
        "Grant confirmation evidence is duplicated, unused, or unrelated.",
    )
}
