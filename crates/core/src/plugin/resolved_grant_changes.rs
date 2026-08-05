use crate::UseResult;

use serde::{Deserialize, Serialize};

use super::grant_changes::{change_set_error, WorkspaceGrantEvidence};
use super::validation::{valid_machine_id, valid_sha256};
use super::{PluginWorkspaceGrant, WorkspaceGrantAuthority, MAX_PLUGIN_PLAN_ITEMS};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedWorkspaceGrantChangeSet {
    pub operation_id: String,
    pub plan_digest: String,
    pub change_set_digest: String,
    pub scope_id: String,
    pub state_revision_before: u64,
    pub revision: u64,
    pub capability_generation_before: u64,
    pub capability_generation_after: u64,
    pub before_snapshot_digest: Option<String>,
    pub transitioned_at_ms: u64,
    pub revocation_authority: WorkspaceGrantAuthority,
    pub grants: Vec<ResolvedWorkspaceGrant>,
    pub revocations: Vec<WorkspaceGrantEvidence>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedWorkspaceGrant {
    pub proposal_digest: String,
    pub grant: PluginWorkspaceGrant,
}

impl ResolvedWorkspaceGrantChangeSet {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.change_set_digest)
            || !valid_machine_id(&self.scope_id)
            || self.state_revision_before == 0
            || self.state_revision_before.checked_add(1) != Some(self.revision)
            || self.capability_generation_before.checked_add(1)
                != Some(self.capability_generation_after)
            || self
                .before_snapshot_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || self.transitioned_at_ms == 0
            || (self.grants.is_empty() && self.revocations.is_empty())
            || self.grants.len() > MAX_PLUGIN_PLAN_ITEMS
            || self.revocations.len() > MAX_PLUGIN_PLAN_ITEMS
            || self
                .grants
                .windows(2)
                .any(|pair| pair[0].grant.package_id >= pair[1].grant.package_id)
            || self
                .revocations
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
            || (!self.revocations.is_empty() && self.before_snapshot_digest.is_none())
        {
            return Err(change_set_error(
                "The resolved workspace grant change-set identity, revision, or ordering is invalid.",
            ));
        }
        self.revocation_authority.validate().map_err(|_| {
            change_set_error("The resolved workspace grant revocation authority is invalid.")
        })?;
        for resolved in &self.grants {
            if !valid_sha256(&resolved.proposal_digest)
                || resolved.grant.scope_id != self.scope_id
                || resolved.grant.validate_at(self.transitioned_at_ms).is_err()
            {
                return Err(change_set_error(
                    "A resolved candidate workspace grant is invalid.",
                ));
            }
        }
        for evidence in &self.revocations {
            evidence.validate().map_err(|_| {
                change_set_error("A resolved workspace grant revocation evidence is invalid.")
            })?;
            if evidence.receipt_revision > self.state_revision_before {
                return Err(change_set_error(
                    "Resolved revocation evidence exceeds the prior state revision.",
                ));
            }
        }
        Ok(())
    }
}
