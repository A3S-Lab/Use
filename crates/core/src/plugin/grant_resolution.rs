use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::plan::MAX_PLAN_LIFETIME_MS;
use super::validation::{valid_machine_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanActor,
    PlanPolicyDecision, PluginPermissionCeiling, PluginWorkspaceGrant, WorkspaceGrantAuthority,
    PLUGIN_GRANT_CONFIRMATION_SCHEMA, PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA,
    PLUGIN_WORKSPACE_GRANT_SCHEMA,
};

const PROPOSAL_ERROR: &str = "use.plugin.grant_proposal_invalid";
const CONFIRMATION_ERROR: &str = "use.plugin.grant_confirmation_invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginWorkspaceGrantProposal {
    pub schema: String,
    pub operation_id: String,
    pub scope_id: String,
    pub package_id: String,
    pub package_digest: String,
    pub permission_ceiling_digest: String,
    pub permissions_digest: String,
    pub permissions: PluginPermissionCeiling,
    pub authority: WorkspaceGrantProposalAuthority,
    pub created_at_ms: u64,
    pub apply_expires_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantProposalAuthority {
    pub actor: PlanActor,
    pub decision: PlanPolicyDecision,
    pub policy_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginGrantConfirmation {
    pub schema: String,
    pub operation_id: String,
    pub plan_digest: String,
    pub proposal_digest: String,
    pub confirmed_by: PlanActor,
    pub confirmed_at_ms: u64,
}

impl PluginWorkspaceGrantProposal {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin workspace grant proposal",
            PROPOSAL_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        self.permissions.validate().map_err(|_| {
            proposal_error("The proposed resolved workspace permissions are invalid.")
        })?;
        PluginWorkspaceGrant::validate_identity(&self.scope_id, &self.package_id)
            .map_err(|_| proposal_error("The grant proposal scope or package is invalid."))?;
        if self.schema != PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.permission_ceiling_digest)
            || !valid_sha256(&self.permissions_digest)
            || self.permissions.surfaces.is_empty()
            || self.permissions.descriptor_digest()? != self.permissions_digest
            || self.created_at_ms == 0
            || self.apply_expires_at_ms <= self.created_at_ms
            || self.apply_expires_at_ms - self.created_at_ms > MAX_PLAN_LIFETIME_MS
            || self
                .grant_expires_at_ms
                .is_some_and(|expires| expires <= self.apply_expires_at_ms)
        {
            return Err(proposal_error(
                "The plugin grant proposal identity, digest, or lifetime is invalid.",
            ));
        }
        self.authority.validate(self.requests_secrets())
    }

    pub fn validate_against(&self, ceiling: &PluginPermissionCeiling) -> UseResult<()> {
        self.validate()?;
        ceiling
            .validate()
            .map_err(|_| proposal_error("The signed package permission ceiling is invalid."))?;
        if ceiling.descriptor_digest()? != self.permission_ceiling_digest {
            return Err(UseError::new(
                "use.plugin.grant_ceiling_mismatch",
                "The workspace grant proposal does not bind the signed permission ceiling.",
            ));
        }
        if !self.permissions.is_within(ceiling)? {
            return Err(UseError::new(
                "use.plugin.grant_exceeds_ceiling",
                "The proposed workspace permissions exceed the signed package ceiling.",
            ));
        }
        Ok(())
    }

    pub fn finalize(
        &self,
        ceiling: &PluginPermissionCeiling,
        plan_digest: &str,
        confirmation: Option<&PluginGrantConfirmation>,
        applied_at_ms: u64,
    ) -> UseResult<PluginWorkspaceGrant> {
        self.validate_against(ceiling)?;
        if !valid_sha256(plan_digest) {
            return Err(proposal_error(
                "The reviewed plugin operation plan digest is invalid.",
            ));
        }
        if applied_at_ms < self.created_at_ms {
            return Err(UseError::new(
                "use.plugin.grant_proposal_not_active",
                "The workspace grant proposal is not active yet.",
            ));
        }
        if applied_at_ms >= self.apply_expires_at_ms {
            return Err(UseError::new(
                "use.plugin.grant_proposal_expired",
                "The workspace grant proposal expired and must be resolved again.",
            ));
        }

        let (granted_at_ms, confirmation_digest) = match self.authority.decision {
            PlanPolicyDecision::Allow if confirmation.is_none() => (applied_at_ms, None),
            PlanPolicyDecision::Allow => {
                return Err(confirmation_mismatch(
                    "An allowed grant proposal must not acquire unrelated confirmation evidence.",
                ));
            }
            PlanPolicyDecision::Ask => {
                let confirmation = confirmation.ok_or_else(|| {
                    UseError::new(
                        "use.plugin.grant_confirmation_required",
                        "The workspace grant proposal requires explicit confirmation.",
                    )
                })?;
                confirmation.validate()?;
                let proposal_digest = self.descriptor_digest()?;
                if confirmation.operation_id != self.operation_id
                    || confirmation.plan_digest != plan_digest
                    || confirmation.proposal_digest != proposal_digest
                    || confirmation.confirmed_at_ms < self.created_at_ms
                    || confirmation.confirmed_at_ms >= self.apply_expires_at_ms
                    || confirmation.confirmed_at_ms > applied_at_ms
                {
                    return Err(confirmation_mismatch(
                        "Confirmation does not bind the exact active plan and grant proposal.",
                    ));
                }
                (
                    confirmation.confirmed_at_ms,
                    Some(confirmation.descriptor_digest()?),
                )
            }
            PlanPolicyDecision::Deny => {
                return Err(proposal_error(
                    "A denied workspace grant proposal cannot be finalized.",
                ));
            }
        };

        let grant = PluginWorkspaceGrant {
            schema: PLUGIN_WORKSPACE_GRANT_SCHEMA.to_string(),
            scope_id: self.scope_id.clone(),
            package_id: self.package_id.clone(),
            package_digest: self.package_digest.clone(),
            permission_ceiling_digest: self.permission_ceiling_digest.clone(),
            permissions_digest: self.permissions_digest.clone(),
            permissions: self.permissions.clone(),
            authority: WorkspaceGrantAuthority {
                actor: self.authority.actor,
                decision: self.authority.decision,
                policy_digest: self.authority.policy_digest.clone(),
                confirmation_digest,
            },
            granted_at_ms,
            expires_at_ms: self.grant_expires_at_ms,
        };
        grant.validate_active_against(ceiling, applied_at_ms)?;
        Ok(grant)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin workspace grant proposal", PROPOSAL_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    fn requests_secrets(&self) -> bool {
        self.permissions
            .surfaces
            .iter()
            .any(|permission| !permission.secrets.is_empty())
    }
}

impl WorkspaceGrantProposalAuthority {
    fn validate(&self, requests_secrets: bool) -> UseResult<()> {
        if !valid_sha256(&self.policy_digest)
            || self.decision == PlanPolicyDecision::Deny
            || (requests_secrets
                && (self.actor != PlanActor::User || self.decision != PlanPolicyDecision::Ask))
        {
            return Err(proposal_error(
                "The workspace grant proposal lacks valid policy authority.",
            ));
        }
        Ok(())
    }
}

impl PluginGrantConfirmation {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin grant confirmation",
            CONFIRMATION_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_GRANT_CONFIRMATION_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.proposal_digest)
            || self.confirmed_by != PlanActor::User
            || self.confirmed_at_ms == 0
        {
            return Err(confirmation_error(
                "The plugin grant confirmation identity, digest, actor, or time is invalid.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin grant confirmation", CONFIRMATION_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

fn proposal_error(message: impl Into<String>) -> UseError {
    contract_error(PROPOSAL_ERROR, message)
}

fn confirmation_error(message: impl Into<String>) -> UseError {
    contract_error(CONFIRMATION_ERROR, message)
}

fn confirmation_mismatch(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.grant_confirmation_mismatch", message)
}
