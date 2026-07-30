use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::validation::{valid_machine_id, valid_package_id, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PlanActor,
    PlanPolicyDecision, PluginPermissionCeiling, PLUGIN_WORKSPACE_GRANT_SCHEMA,
};

const GRANT_ERROR: &str = "use.plugin.grant_invalid";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginWorkspaceGrant {
    pub schema: String,
    pub scope_id: String,
    pub package_id: String,
    pub package_digest: String,
    pub permission_ceiling_digest: String,
    pub permissions_digest: String,
    pub permissions: PluginPermissionCeiling,
    pub authority: WorkspaceGrantAuthority,
    pub granted_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantAuthority {
    pub actor: PlanActor,
    pub decision: PlanPolicyDecision,
    pub policy_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmation_digest: Option<String>,
}

impl PluginWorkspaceGrant {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(input, "plugin workspace grant", GRANT_ERROR, Self::validate)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.permissions
            .validate()
            .map_err(|_| grant_error("The resolved workspace permissions are invalid."))?;
        if self.schema != PLUGIN_WORKSPACE_GRANT_SCHEMA
            || Self::validate_identity(&self.scope_id, &self.package_id).is_err()
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.permission_ceiling_digest)
            || !valid_sha256(&self.permissions_digest)
            || self.permissions.surfaces.is_empty()
            || self.permissions.descriptor_digest()? != self.permissions_digest
            || self.granted_at_ms == 0
            || self
                .expires_at_ms
                .is_some_and(|expires| expires <= self.granted_at_ms)
        {
            return Err(grant_error(
                "The plugin workspace grant identity, digest, or lifetime is invalid.",
            ));
        }
        self.authority.validate()?;
        if self.requests_secrets()
            && (self.authority.actor != PlanActor::User
                || self.authority.decision != PlanPolicyDecision::Ask)
        {
            return Err(grant_error(
                "Secret-bearing workspace grants require explicit user confirmation.",
            ));
        }
        Ok(())
    }

    pub fn validate_identity(scope_id: &str, package_id: &str) -> UseResult<()> {
        Self::validate_scope_id(scope_id)?;
        Self::validate_package_id(package_id)?;
        Ok(())
    }

    pub fn validate_package_id(package_id: &str) -> UseResult<()> {
        if !valid_package_id(package_id) {
            return Err(grant_error(
                "The plugin workspace grant package identity is invalid.",
            ));
        }
        Ok(())
    }

    pub fn validate_scope_id(scope_id: &str) -> UseResult<()> {
        if !valid_machine_id(scope_id) {
            return Err(grant_error(
                "The plugin workspace grant scope identity is invalid.",
            ));
        }
        Ok(())
    }

    pub fn validate_against(&self, ceiling: &PluginPermissionCeiling) -> UseResult<()> {
        self.validate()?;
        ceiling
            .validate()
            .map_err(|_| grant_error("The signed package permission ceiling is invalid."))?;
        if ceiling.descriptor_digest()? != self.permission_ceiling_digest {
            return Err(UseError::new(
                "use.plugin.grant_ceiling_mismatch",
                "The workspace grant does not bind the signed package permission ceiling.",
            ));
        }
        if !self.permissions.is_within(ceiling)? {
            return Err(UseError::new(
                "use.plugin.grant_exceeds_ceiling",
                "The resolved workspace grant exceeds the signed package permission ceiling.",
            ));
        }
        Ok(())
    }

    pub fn validate_at(&self, now_ms: u64) -> UseResult<()> {
        self.validate()?;
        if now_ms < self.granted_at_ms {
            return Err(UseError::new(
                "use.plugin.grant_not_active",
                "The workspace grant is not active yet.",
            ));
        }
        if self.expires_at_ms.is_some_and(|expires| now_ms >= expires) {
            return Err(UseError::new(
                "use.plugin.grant_expired",
                "The workspace grant expired and must be reviewed again.",
            ));
        }
        Ok(())
    }

    pub fn validate_active_against(
        &self,
        ceiling: &PluginPermissionCeiling,
        now_ms: u64,
    ) -> UseResult<()> {
        self.validate_against(ceiling)?;
        self.validate_at(now_ms)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin workspace grant", GRANT_ERROR)
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

impl WorkspaceGrantAuthority {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_sha256(&self.policy_digest)
            || self
                .confirmation_digest
                .as_deref()
                .is_some_and(|digest| !valid_sha256(digest))
            || matches!(self.decision, PlanPolicyDecision::Deny)
            || (self.decision == PlanPolicyDecision::Ask) != self.confirmation_digest.is_some()
        {
            return Err(grant_error(
                "The workspace grant authority lacks valid policy or confirmation evidence.",
            ));
        }
        Ok(())
    }
}

fn grant_error(message: impl Into<String>) -> UseError {
    contract_error(GRANT_ERROR, message)
}
