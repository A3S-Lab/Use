use std::collections::BTreeSet;

use a3s_use_core::{
    PlanPackageChangeKind, PlanPolicyDecision, PluginOperationPlanEnvelope,
    ResolvedWorkspaceGrantChangeSet, UseError, UseResult,
};
use a3s_use_extension::{
    WorkspaceGrantCandidateCeiling, WorkspaceGrantCutoverEvidence, WorkspaceGrantLifecyclePhase,
    WorkspaceGrantOperationJournal, WorkspaceGrantStore, WORKSPACE_GRANT_CUTOVER_SCHEMA,
};

use super::model::valid_sha256;

/// Exact capability snapshot selected by one graph cutover.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginCapabilityCutoverEvidence {
    capability_generation_before: u64,
    capability_generation_after: u64,
    capability_snapshot_digest: String,
}

impl PluginCapabilityCutoverEvidence {
    pub fn new(
        capability_generation_before: u64,
        capability_generation_after: u64,
        capability_snapshot_digest: impl Into<String>,
    ) -> UseResult<Self> {
        let evidence = Self {
            capability_generation_before,
            capability_generation_after,
            capability_snapshot_digest: capability_snapshot_digest.into(),
        };
        evidence.validate()?;
        Ok(evidence)
    }

    pub fn capability_generation_before(&self) -> u64 {
        self.capability_generation_before
    }

    pub fn capability_generation_after(&self) -> u64 {
        self.capability_generation_after
    }

    pub fn capability_snapshot_digest(&self) -> &str {
        &self.capability_snapshot_digest
    }

    fn validate(&self) -> UseResult<()> {
        if self.capability_generation_before.checked_add(1)
            != Some(self.capability_generation_after)
            || !valid_sha256(&self.capability_snapshot_digest)
        {
            return Err(composition_error(
                "Capability cutover evidence has invalid generation or snapshot identity.",
            ));
        }
        Ok(())
    }

    fn validate_against(&self, resolved: &ResolvedWorkspaceGrantChangeSet) -> UseResult<()> {
        self.validate()?;
        if self.capability_generation_before != resolved.capability_generation_before
            || self.capability_generation_after != resolved.capability_generation_after
        {
            return Err(composition_error(
                "Capability cutover evidence drifted from the plan-bound grant generation.",
            ));
        }
        Ok(())
    }
}

/// One plan-bound grant sub-saga composed around package graph publication.
///
/// The resolved grants and exact signed ceilings are immutable for the unit's
/// lifetime. Candidate receipts are durable before package or Runtime prepare;
/// prior receipts can retire only after an exact capability snapshot cutover.
#[derive(Debug, Clone)]
pub struct PluginGrantLifecycleUnit {
    store: WorkspaceGrantStore,
    envelope: PluginOperationPlanEnvelope,
    resolved: ResolvedWorkspaceGrantChangeSet,
    ceilings: Vec<WorkspaceGrantCandidateCeiling>,
}

impl PluginGrantLifecycleUnit {
    pub fn new(
        store: WorkspaceGrantStore,
        envelope: PluginOperationPlanEnvelope,
        resolved: ResolvedWorkspaceGrantChangeSet,
        ceilings: Vec<WorkspaceGrantCandidateCeiling>,
    ) -> UseResult<Self> {
        validate_binding(&envelope, &resolved, &ceilings)?;
        Ok(Self {
            store,
            envelope,
            resolved,
            ceilings,
        })
    }

    pub fn operation_id(&self) -> &str {
        &self.resolved.operation_id
    }

    pub fn resolved(&self) -> &ResolvedWorkspaceGrantChangeSet {
        &self.resolved
    }

    pub fn validate_envelope(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        if &self.envelope != envelope {
            return Err(composition_error(
                "The grant lifecycle unit does not belong to this immutable package plan.",
            ));
        }
        validate_binding(envelope, &self.resolved, &self.ceilings)
    }

    pub async fn prepare(&self, now_ms: u64) -> UseResult<WorkspaceGrantOperationJournal> {
        self.store
            .begin_change_set(&self.resolved, &self.ceilings)
            .await?;
        self.store
            .prepare_change_set(&self.resolved.operation_id, now_ms)
            .await
    }

    pub async fn commit_cutover(
        &self,
        evidence: &PluginCapabilityCutoverEvidence,
        committed_at_ms: u64,
        now_ms: u64,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        evidence.validate_against(&self.resolved)?;
        let cutover = match self.observe().await?.and_then(|journal| journal.cutover) {
            Some(recorded) => {
                if recorded.capability_generation_before != evidence.capability_generation_before
                    || recorded.capability_generation_after != evidence.capability_generation_after
                    || recorded.capability_snapshot_digest != evidence.capability_snapshot_digest
                {
                    return Err(composition_error(
                        "Replayed capability cutover evidence drifted from the durable grant checkpoint.",
                    ));
                }
                recorded
            }
            None => WorkspaceGrantCutoverEvidence {
                schema: WORKSPACE_GRANT_CUTOVER_SCHEMA.to_string(),
                capability_generation_before: evidence.capability_generation_before,
                capability_generation_after: evidence.capability_generation_after,
                capability_snapshot_digest: evidence.capability_snapshot_digest.clone(),
                committed_at_ms,
            },
        };
        self.store
            .commit_change_set_cutover(&self.resolved.operation_id, cutover, now_ms)
            .await
    }

    pub async fn retire(&self) -> UseResult<WorkspaceGrantOperationJournal> {
        self.store
            .retire_change_set(&self.resolved.operation_id)
            .await
    }

    pub async fn rollback(
        &self,
        evidence_digest: impl Into<String>,
        rolled_back_at_ms: u64,
        now_ms: u64,
    ) -> UseResult<WorkspaceGrantOperationJournal> {
        let requested_digest = evidence_digest.into();
        let (evidence_digest, rolled_back_at_ms) = match self.observe().await? {
            Some(journal) => match journal.rollback {
                Some(rollback) => (rollback.evidence_digest, rollback.rolled_back_at_ms),
                None => (requested_digest, rolled_back_at_ms),
            },
            None => (requested_digest, rolled_back_at_ms),
        };
        self.store
            .rollback_change_set(
                &self.resolved.operation_id,
                evidence_digest,
                rolled_back_at_ms,
                now_ms,
            )
            .await
    }

    pub async fn observe(&self) -> UseResult<Option<WorkspaceGrantOperationJournal>> {
        self.store
            .observe_change_set(&self.resolved.operation_id)
            .await
    }

    pub async fn is_rolled_back(&self) -> UseResult<bool> {
        Ok(self.observe().await?.is_some_and(|journal| {
            matches!(
                journal.phase,
                WorkspaceGrantLifecyclePhase::RollingBack
                    | WorkspaceGrantLifecyclePhase::RolledBack
            )
        }))
    }

    pub async fn has_cutover(&self) -> UseResult<bool> {
        Ok(self.observe().await?.is_some_and(|journal| {
            matches!(
                journal.phase,
                WorkspaceGrantLifecyclePhase::CutoverCommitted
                    | WorkspaceGrantLifecyclePhase::Retiring
                    | WorkspaceGrantLifecyclePhase::Completed
            )
        }))
    }
}

fn validate_binding(
    envelope: &PluginOperationPlanEnvelope,
    resolved: &ResolvedWorkspaceGrantChangeSet,
    ceilings: &[WorkspaceGrantCandidateCeiling],
) -> UseResult<()> {
    envelope.validate()?;
    resolved.validate()?;
    let plan = &envelope.plan;
    let impact = plan
        .workspace_impacts
        .iter()
        .find(|impact| impact.scope_id == resolved.scope_id)
        .ok_or_else(|| {
            composition_error("The package plan omits the resolved grant workspace impact.")
        })?;
    if resolved.operation_id != plan.operation_id
        || resolved.plan_digest != envelope.plan_digest
        || resolved.state_revision_before != plan.state.state_revision
        || resolved.capability_generation_before != plan.state.capability_generation
        || impact.grant_before_digest != resolved.before_snapshot_digest
        || impact.grant_after_digest.as_deref() != Some(&resolved.change_set_digest)
        || resolved.transitioned_at_ms < plan.created_at_ms
        || resolved.transitioned_at_ms >= plan.expires_at_ms
        || resolved.revocation_authority.actor != plan.authority.actor
        || resolved.revocation_authority.decision != plan.authority.decision
        || resolved.revocation_authority.policy_digest != plan.authority.policy_digest
        || (plan.authority.decision == PlanPolicyDecision::Ask)
            != resolved.revocation_authority.confirmation_digest.is_some()
    {
        return Err(composition_error(
            "The resolved grant change set drifted from its reviewed package plan.",
        ));
    }

    let expected_candidates = plan
        .packages
        .iter()
        .filter(|package| {
            impact.enabled_after
                && matches!(
                    package.change,
                    PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
                )
                && package
                    .after
                    .as_ref()
                    .is_some_and(|state| !state.permissions.surfaces.is_empty())
        })
        .map(|package| package.package_id.as_str())
        .collect::<BTreeSet<_>>();
    let actual_candidates = resolved
        .grants
        .iter()
        .map(|candidate| candidate.grant.package_id.as_str())
        .collect::<BTreeSet<_>>();
    let expected_retirements = plan
        .packages
        .iter()
        .filter(|package| {
            impact.enabled_before
                && matches!(
                    package.change,
                    PlanPackageChangeKind::Remove | PlanPackageChangeKind::Replace
                )
                && package
                    .before
                    .as_ref()
                    .is_some_and(|state| !state.permissions.surfaces.is_empty())
        })
        .map(|package| package.package_id.as_str())
        .collect::<BTreeSet<_>>();
    let actual_retirements = resolved
        .revocations
        .iter()
        .map(|evidence| evidence.package_id.as_str())
        .collect::<BTreeSet<_>>();
    if expected_candidates != actual_candidates
        || expected_retirements != actual_retirements
        || actual_candidates.len() != resolved.grants.len()
        || actual_retirements.len() != resolved.revocations.len()
        || ceilings.len() != resolved.grants.len()
    {
        return Err(composition_error(
            "The resolved grants do not cover the exact permission-bearing package transition.",
        ));
    }

    for ((candidate, ceiling), package_id) in resolved
        .grants
        .iter()
        .zip(ceilings)
        .zip(expected_candidates.iter())
    {
        ceiling.validate()?;
        let state = plan
            .packages
            .iter()
            .find(|package| package.package_id == *package_id)
            .and_then(|package| package.after.as_ref())
            .ok_or_else(|| composition_error("A candidate grant has no planned package state."))?;
        if candidate.grant.package_id != *package_id
            || candidate.grant.scope_id != resolved.scope_id
            || candidate.grant.package_digest != state.release.package_sha256
            || candidate.grant.permission_ceiling_digest != state.release.permission_ceiling_digest
            || candidate.grant.authority.actor != plan.authority.actor
            || candidate.grant.authority.decision != plan.authority.decision
            || candidate.grant.authority.policy_digest != plan.authority.policy_digest
            || ceiling.package_id != *package_id
            || ceiling.package_digest != state.release.package_sha256
            || ceiling.ceiling != state.permissions
        {
            return Err(composition_error(
                "A candidate grant or signed ceiling drifted from its planned package generation.",
            ));
        }
        candidate.grant.validate_against(&state.permissions)?;
    }

    for retirement in &resolved.revocations {
        let state = plan
            .packages
            .iter()
            .find(|package| package.package_id == retirement.package_id)
            .and_then(|package| package.before.as_ref())
            .ok_or_else(|| composition_error("A grant retirement has no prior package state."))?;
        if retirement.package_digest != state.release.package_sha256 {
            return Err(composition_error(
                "A grant retirement drifted from its exact prior package generation.",
            ));
        }
    }
    Ok(())
}

fn composition_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.grant_composition_invalid", message)
}
