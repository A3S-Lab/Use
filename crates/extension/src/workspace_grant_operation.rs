use a3s_use_core::{
    PluginOperationPlan, PluginPermissionCeiling, PluginWorkspaceGrant,
    ResolvedWorkspaceGrantChangeSet, UseError, UseResult, WorkspaceGrantAuthority,
    WorkspaceGrantEvidence, MAX_PLUGIN_PLAN_ITEMS,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::workspace_grant::{
    valid_sha256, StoredWorkspaceGrant, WorkspaceGrantReceipt, WorkspaceGrantStore,
};

pub const WORKSPACE_GRANT_OPERATION_SCHEMA: &str = "a3s.use.plugin-workspace-grant-operation.v1";
pub const WORKSPACE_GRANT_CUTOVER_SCHEMA: &str = "a3s.use.plugin-workspace-grant-cutover.v1";
pub const WORKSPACE_GRANT_ROLLBACK_SCHEMA: &str = "a3s.use.plugin-workspace-grant-rollback.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceGrantLifecyclePhase {
    IntentRecorded,
    Preparing,
    Prepared,
    CutoverCommitted,
    Retiring,
    Completed,
    RollingBack,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantCandidateCeiling {
    pub package_id: String,
    pub package_digest: String,
    pub ceiling: PluginPermissionCeiling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantPreparedCandidate {
    pub proposal_digest: String,
    pub receipt: WorkspaceGrantReceipt,
    pub ceiling: PluginPermissionCeiling,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prior_record: Option<StoredWorkspaceGrant>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantRetirement {
    pub evidence: WorkspaceGrantEvidence,
    pub prior_receipt: WorkspaceGrantReceipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantOperationIntent {
    pub operation_id: String,
    pub plan_digest: String,
    pub change_set_digest: String,
    pub scope_id: String,
    pub state_revision_before: u64,
    pub revision: u64,
    pub capability_generation_before: u64,
    pub capability_generation_after: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_snapshot_digest: Option<String>,
    pub observed_before_snapshot_digest: String,
    pub transitioned_at_ms: u64,
    pub revocation_authority: WorkspaceGrantAuthority,
    pub candidates: Vec<WorkspaceGrantPreparedCandidate>,
    pub retirements: Vec<WorkspaceGrantRetirement>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantCutoverEvidence {
    pub schema: String,
    pub capability_generation_before: u64,
    pub capability_generation_after: u64,
    pub capability_snapshot_digest: String,
    pub committed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantRollbackEvidence {
    pub schema: String,
    pub evidence_digest: String,
    pub rolled_back_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantOperationJournal {
    pub schema: String,
    pub intent_digest: String,
    pub intent: WorkspaceGrantOperationIntent,
    pub phase: WorkspaceGrantLifecyclePhase,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cutover: Option<WorkspaceGrantCutoverEvidence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rollback: Option<WorkspaceGrantRollbackEvidence>,
}

impl WorkspaceGrantCandidateCeiling {
    pub fn validate(&self) -> UseResult<()> {
        PluginWorkspaceGrant::validate_package_id(&self.package_id)
            .map_err(|_| operation_error("A candidate ceiling package identity is invalid."))?;
        self.ceiling
            .validate()
            .map_err(|_| operation_error("A candidate permission ceiling is invalid."))?;
        if !valid_sha256(&self.package_digest) {
            return Err(operation_error(
                "A candidate ceiling package digest is invalid.",
            ));
        }
        Ok(())
    }
}

impl WorkspaceGrantPreparedCandidate {
    fn validate(&self, scope_id: &str, revision: u64, transitioned_at_ms: u64) -> UseResult<()> {
        self.receipt.validate()?;
        if !valid_sha256(&self.proposal_digest)
            || self.receipt.revision != revision
            || self.receipt.grant.scope_id != scope_id
        {
            return Err(operation_error(
                "A prepared workspace grant candidate has invalid identity or revision.",
            ));
        }
        self.receipt
            .grant
            .validate_active_against(&self.ceiling, transitioned_at_ms)
            .map_err(|_| {
                operation_error(
                    "A prepared workspace grant candidate exceeds its ceiling or is inactive.",
                )
            })?;
        if let Some(prior) = &self.prior_record {
            prior.validate()?;
            if prior.scope_id() != scope_id
                || prior.package_id() != self.receipt.grant.package_id
                || prior.package_digest() != self.receipt.grant.package_digest
                || prior.revision() >= revision
            {
                return Err(operation_error(
                    "A prepared workspace grant candidate has invalid rollback ownership evidence.",
                ));
            }
        }
        Ok(())
    }

    fn package_id(&self) -> &str {
        &self.receipt.grant.package_id
    }
}

impl WorkspaceGrantRetirement {
    fn validate(&self, scope_id: &str, state_revision_before: u64) -> UseResult<()> {
        self.evidence.validate()?;
        self.prior_receipt.validate()?;
        if self.prior_receipt.grant.scope_id != scope_id
            || self.evidence.package_id != self.prior_receipt.grant.package_id
            || self.evidence.package_digest != self.prior_receipt.grant.package_digest
            || self.evidence.receipt_revision != self.prior_receipt.revision
            || self.evidence.grant_digest != self.prior_receipt.grant_digest
            || self.prior_receipt.revision > state_revision_before
        {
            return Err(operation_error(
                "A workspace grant retirement does not bind its exact prior receipt.",
            ));
        }
        Ok(())
    }

    fn package_id(&self) -> &str {
        &self.evidence.package_id
    }
}

impl WorkspaceGrantOperationIntent {
    pub fn validate(&self) -> UseResult<()> {
        PluginOperationPlan::validate_operation_id(&self.operation_id)?;
        PluginWorkspaceGrant::validate_scope_id(&self.scope_id)?;
        self.revocation_authority.validate()?;
        if !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.change_set_digest)
            || self.state_revision_before == 0
            || self.state_revision_before.checked_add(1) != Some(self.revision)
            || self.capability_generation_before.checked_add(1)
                != Some(self.capability_generation_after)
            || self
                .before_snapshot_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || !valid_sha256(&self.observed_before_snapshot_digest)
            || self.transitioned_at_ms == 0
            || (self.candidates.is_empty() && self.retirements.is_empty())
            || self.candidates.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.retirements.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .candidates
                .windows(2)
                .any(|pair| pair[0].package_id() >= pair[1].package_id())
            || self
                .retirements
                .windows(2)
                .any(|pair| pair[0].package_id() >= pair[1].package_id())
            || (!self.retirements.is_empty() && self.before_snapshot_digest.is_none())
        {
            return Err(operation_error(
                "A workspace grant operation intent has invalid identity, revision, or ordering.",
            ));
        }
        for candidate in &self.candidates {
            candidate.validate(&self.scope_id, self.revision, self.transitioned_at_ms)?;
            if let Some(StoredWorkspaceGrant::Granted(prior)) = &candidate.prior_record {
                let exact_retirement = self
                    .retirements
                    .iter()
                    .any(|retirement| retirement.prior_receipt == *prior);
                if !exact_retirement {
                    return Err(operation_error(
                        "A candidate replacing an active same-generation grant omitted its exact retirement evidence.",
                    ));
                }
            }
        }
        for retirement in &self.retirements {
            retirement.validate(&self.scope_id, self.state_revision_before)?;
        }
        Ok(())
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        let bytes = serde_json::to_vec(self).map_err(|error| {
            operation_error(format!(
                "Failed to encode workspace grant operation intent: {error}"
            ))
        })?;
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

impl WorkspaceGrantCutoverEvidence {
    pub fn validate_against(&self, intent: &WorkspaceGrantOperationIntent) -> UseResult<()> {
        if self.schema != WORKSPACE_GRANT_CUTOVER_SCHEMA
            || self.capability_generation_before != intent.capability_generation_before
            || self.capability_generation_after != intent.capability_generation_after
            || !valid_sha256(&self.capability_snapshot_digest)
            || self.committed_at_ms < intent.transitioned_at_ms
        {
            return Err(operation_error(
                "Capability cutover evidence does not bind the prepared grant operation.",
            ));
        }
        Ok(())
    }
}

impl WorkspaceGrantRollbackEvidence {
    pub fn validate_against(&self, intent: &WorkspaceGrantOperationIntent) -> UseResult<()> {
        if self.schema != WORKSPACE_GRANT_ROLLBACK_SCHEMA
            || !valid_sha256(&self.evidence_digest)
            || self.rolled_back_at_ms < intent.transitioned_at_ms
        {
            return Err(operation_error(
                "Candidate rollback evidence does not bind the prepared grant operation.",
            ));
        }
        Ok(())
    }
}

impl WorkspaceGrantOperationJournal {
    pub(super) fn new(intent: WorkspaceGrantOperationIntent) -> UseResult<Self> {
        let intent_digest = intent.descriptor_digest()?;
        let journal = Self {
            schema: WORKSPACE_GRANT_OPERATION_SCHEMA.to_string(),
            intent_digest,
            intent,
            phase: WorkspaceGrantLifecyclePhase::IntentRecorded,
            cutover: None,
            rollback: None,
        };
        journal.validate()?;
        Ok(journal)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.intent.validate()?;
        let cutover_required = matches!(
            self.phase,
            WorkspaceGrantLifecyclePhase::CutoverCommitted
                | WorkspaceGrantLifecyclePhase::Retiring
                | WorkspaceGrantLifecyclePhase::Completed
        );
        let rollback_required = matches!(
            self.phase,
            WorkspaceGrantLifecyclePhase::RollingBack | WorkspaceGrantLifecyclePhase::RolledBack
        );
        if self.schema != WORKSPACE_GRANT_OPERATION_SCHEMA
            || !valid_sha256(&self.intent_digest)
            || self.intent.descriptor_digest()? != self.intent_digest
            || cutover_required != self.cutover.is_some()
            || rollback_required != self.rollback.is_some()
            || self.cutover.is_some() && self.rollback.is_some()
        {
            return Err(operation_error(
                "A workspace grant operation journal has invalid schema, digest, or phase evidence.",
            ));
        }
        if let Some(cutover) = &self.cutover {
            cutover.validate_against(&self.intent)?;
        }
        if let Some(rollback) = &self.rollback {
            rollback.validate_against(&self.intent)?;
        }
        Ok(())
    }
}

impl WorkspaceGrantStore {
    pub(super) fn operation_path(&self, operation_id: &str) -> UseResult<std::path::PathBuf> {
        PluginOperationPlan::validate_operation_id(operation_id).map_err(|_| {
            operation_error("A workspace grant operation path identity is invalid.")
        })?;
        let operation_digest = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
        Ok(self
            .root()
            .join(".operations")
            .join(format!("{operation_digest}.json")))
    }
}

pub(super) fn operation_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.grant_operation.invalid", message)
}

pub(super) fn operation_state_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

pub(super) fn validate_resolved(resolved: &ResolvedWorkspaceGrantChangeSet) -> UseResult<()> {
    resolved
        .validate()
        .map_err(|_| operation_error("The resolved workspace grant change set is invalid."))
}
