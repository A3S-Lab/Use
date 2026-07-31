use std::path::{Path, PathBuf};

use a3s_use_core::{
    PluginPermissionCeiling, PluginWorkspaceGrant, UseError, UseResult, WorkspaceGrantAuthority,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::workspace_grant_io::{
    acquire_lock, ensure_owned_directory, read_optional_record, validate_existing_directory_chain,
    write_record,
};
use super::ExtensionPaths;

pub const WORKSPACE_GRANT_RECEIPT_SCHEMA: &str = "a3s.use.plugin-workspace-grant-receipt.v1";
pub const WORKSPACE_GRANT_REVOCATION_SCHEMA: &str = "a3s.use.plugin-workspace-grant-revocation.v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    content = "record",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
pub enum StoredWorkspaceGrant {
    Granted(WorkspaceGrantReceipt),
    Revoked(WorkspaceGrantRevocation),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantReceipt {
    pub schema: String,
    pub revision: u64,
    pub grant_digest: String,
    pub grant: PluginWorkspaceGrant,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceGrantRevocation {
    pub schema: String,
    pub scope_id: String,
    pub package_id: String,
    pub package_digest: String,
    pub revision: u64,
    pub prior_revision: u64,
    pub prior_grant_digest: String,
    pub authority: WorkspaceGrantAuthority,
    pub revoked_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceGrantStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl WorkspaceGrantReceipt {
    pub fn new(revision: u64, grant: PluginWorkspaceGrant) -> UseResult<Self> {
        grant.validate()?;
        let receipt = Self {
            schema: WORKSPACE_GRANT_RECEIPT_SCHEMA.to_string(),
            revision,
            grant_digest: grant.descriptor_digest()?,
            grant,
        };
        receipt.validate()?;
        Ok(receipt)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.grant.validate()?;
        if self.schema != WORKSPACE_GRANT_RECEIPT_SCHEMA
            || self.revision == 0
            || !valid_sha256(&self.grant_digest)
            || self.grant.descriptor_digest()? != self.grant_digest
        {
            return Err(record_error(
                "A workspace grant receipt has invalid schema, revision, or digest evidence.",
            ));
        }
        Ok(())
    }
}

impl WorkspaceGrantRevocation {
    pub fn new(
        revision: u64,
        prior: &WorkspaceGrantReceipt,
        authority: WorkspaceGrantAuthority,
        revoked_at_ms: u64,
    ) -> UseResult<Self> {
        prior.validate()?;
        let revocation = Self {
            schema: WORKSPACE_GRANT_REVOCATION_SCHEMA.to_string(),
            scope_id: prior.grant.scope_id.clone(),
            package_id: prior.grant.package_id.clone(),
            package_digest: prior.grant.package_digest.clone(),
            revision,
            prior_revision: prior.revision,
            prior_grant_digest: prior.grant_digest.clone(),
            authority,
            revoked_at_ms,
        };
        revocation.validate()?;
        validate_revocation_transition(prior, &revocation)?;
        Ok(revocation)
    }

    pub fn validate(&self) -> UseResult<()> {
        PluginWorkspaceGrant::validate_identity(&self.scope_id, &self.package_id)
            .map_err(|_| record_error("A workspace grant revocation identity is invalid."))?;
        self.authority.validate()?;
        if self.schema != WORKSPACE_GRANT_REVOCATION_SCHEMA
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.prior_grant_digest)
            || self.prior_revision == 0
            || self.revision <= self.prior_revision
            || self.revoked_at_ms == 0
        {
            return Err(record_error(
                "A workspace grant revocation has invalid schema, revision, or digest evidence.",
            ));
        }
        Ok(())
    }
}

impl StoredWorkspaceGrant {
    pub fn validate(&self) -> UseResult<()> {
        match self {
            Self::Granted(receipt) => receipt.validate(),
            Self::Revoked(revocation) => revocation.validate(),
        }
    }

    pub fn scope_id(&self) -> &str {
        match self {
            Self::Granted(receipt) => &receipt.grant.scope_id,
            Self::Revoked(revocation) => &revocation.scope_id,
        }
    }

    pub fn package_id(&self) -> &str {
        match self {
            Self::Granted(receipt) => &receipt.grant.package_id,
            Self::Revoked(revocation) => &revocation.package_id,
        }
    }

    pub fn package_digest(&self) -> &str {
        match self {
            Self::Granted(receipt) => &receipt.grant.package_digest,
            Self::Revoked(revocation) => &revocation.package_digest,
        }
    }

    pub fn revision(&self) -> u64 {
        match self {
            Self::Granted(receipt) => receipt.revision,
            Self::Revoked(revocation) => revocation.revision,
        }
    }

    fn transition_time_ms(&self) -> u64 {
        match self {
            Self::Granted(receipt) => receipt.grant.granted_at_ms,
            Self::Revoked(revocation) => revocation.revoked_at_ms,
        }
    }
}

impl WorkspaceGrantStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("grants"),
            state_root,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::new(paths.state_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub(super) fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub async fn put(
        &self,
        receipt: &WorkspaceGrantReceipt,
        ceiling: &PluginPermissionCeiling,
        now_ms: u64,
    ) -> UseResult<bool> {
        let _lock = acquire_lock(&self.state_root, &self.root).await?;
        self.put_locked(receipt, ceiling, now_ms).await
    }

    pub(super) async fn put_locked(
        &self,
        receipt: &WorkspaceGrantReceipt,
        ceiling: &PluginPermissionCeiling,
        now_ms: u64,
    ) -> UseResult<bool> {
        receipt.validate()?;
        receipt.grant.validate_active_against(ceiling, now_ms)?;
        let path = self.record_path(
            &receipt.grant.scope_id,
            &receipt.grant.package_id,
            &receipt.grant.package_digest,
        )?;
        ensure_owned_directory(&self.root, path.parent()).await?;
        if let Some(current) = read_optional_record(&path).await? {
            if current == StoredWorkspaceGrant::Granted(receipt.clone()) {
                return Ok(false);
            }
            if current.scope_id() != receipt.grant.scope_id
                || current.package_id() != receipt.grant.package_id
                || current.package_digest() != receipt.grant.package_digest
            {
                return Err(store_error(
                    "use.plugin.grant_store.ownership_mismatch",
                    "A workspace grant record does not match its scope, package, and generation path.",
                ));
            }
            validate_grant_transition(&current, receipt)?;
        }
        write_record(&path, &StoredWorkspaceGrant::Granted(receipt.clone())).await?;
        Ok(true)
    }

    pub async fn observe(
        &self,
        scope_id: &str,
        package_id: &str,
        package_digest: &str,
    ) -> UseResult<Option<StoredWorkspaceGrant>> {
        self.observe_record(scope_id, package_id, package_digest)
            .await
    }

    pub(super) async fn observe_record(
        &self,
        scope_id: &str,
        package_id: &str,
        package_digest: &str,
    ) -> UseResult<Option<StoredWorkspaceGrant>> {
        let path = self.record_path(scope_id, package_id, package_digest)?;
        if !validate_existing_directory_chain(&self.state_root, path.parent()).await? {
            return Ok(None);
        }
        let Some(record) = read_optional_record(&path).await? else {
            return Ok(None);
        };
        if record.scope_id() != scope_id
            || record.package_id() != package_id
            || record.package_digest() != package_digest
        {
            return Err(store_error(
                "use.plugin.grant_store.ownership_mismatch",
                "A workspace grant record does not match its scope, package, and generation path.",
            ));
        }
        Ok(Some(record))
    }

    pub async fn resolve_active(
        &self,
        scope_id: &str,
        package_id: &str,
        package_digest: &str,
        ceiling: &PluginPermissionCeiling,
        now_ms: u64,
    ) -> UseResult<Option<WorkspaceGrantReceipt>> {
        let Some(record) = self.observe(scope_id, package_id, package_digest).await? else {
            return Ok(None);
        };
        let StoredWorkspaceGrant::Granted(receipt) = record else {
            return Ok(None);
        };
        receipt.grant.validate_active_against(ceiling, now_ms)?;
        Ok(Some(receipt))
    }

    pub async fn revoke(
        &self,
        expected: &WorkspaceGrantReceipt,
        revocation: &WorkspaceGrantRevocation,
    ) -> UseResult<bool> {
        let _lock = acquire_lock(&self.state_root, &self.root).await?;
        self.revoke_locked(expected, revocation).await
    }

    pub(super) async fn revoke_locked(
        &self,
        expected: &WorkspaceGrantReceipt,
        revocation: &WorkspaceGrantRevocation,
    ) -> UseResult<bool> {
        expected.validate()?;
        revocation.validate()?;
        validate_revocation_transition(expected, revocation)?;
        let path = self.record_path(
            &expected.grant.scope_id,
            &expected.grant.package_id,
            &expected.grant.package_digest,
        )?;
        ensure_owned_directory(&self.root, path.parent()).await?;
        let Some(current) = read_optional_record(&path).await? else {
            return Err(ownership_changed());
        };
        if current == StoredWorkspaceGrant::Revoked(revocation.clone()) {
            return Ok(false);
        }
        if current != StoredWorkspaceGrant::Granted(expected.clone()) {
            return Err(ownership_changed());
        }
        write_record(&path, &StoredWorkspaceGrant::Revoked(revocation.clone())).await?;
        Ok(true)
    }

    pub(super) fn record_path(
        &self,
        scope_id: &str,
        package_id: &str,
        package_digest: &str,
    ) -> UseResult<PathBuf> {
        PluginWorkspaceGrant::validate_identity(scope_id, package_id).map_err(|_| {
            store_error(
                "use.plugin.grant_store.path_invalid",
                "A workspace grant scope or package path identity is invalid.",
            )
        })?;
        if !valid_sha256(package_digest) {
            return Err(path_error());
        }
        let generation_digest = package_digest
            .strip_prefix("sha256:")
            .ok_or_else(path_error)?;
        let mut segments = package_id.split('/');
        let publisher = segments.next().ok_or_else(path_error)?;
        let package = segments.next().ok_or_else(path_error)?;
        Ok(self
            .scope_path(scope_id)?
            .join(publisher)
            .join(package)
            .join(format!("{generation_digest}.json")))
    }

    pub(super) fn scope_path(&self, scope_id: &str) -> UseResult<PathBuf> {
        PluginWorkspaceGrant::validate_scope_id(scope_id).map_err(|_| {
            store_error(
                "use.plugin.grant_store.path_invalid",
                "A workspace grant scope path identity is invalid.",
            )
        })?;
        let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
        Ok(self.root.join(scope_digest))
    }
}

fn validate_grant_transition(
    current: &StoredWorkspaceGrant,
    next: &WorkspaceGrantReceipt,
) -> UseResult<()> {
    if current.revision() > next.revision || current.transition_time_ms() > next.grant.granted_at_ms
    {
        return Err(store_error(
            "use.plugin.grant_store.stale",
            "A stale workspace grant cannot replace the current authorization state.",
        ));
    }
    if current.revision() == next.revision {
        return Err(store_error(
            "use.plugin.grant_store.conflict",
            "A workspace grant revision has conflicting immutable content.",
        ));
    }
    Ok(())
}

fn validate_revocation_transition(
    prior: &WorkspaceGrantReceipt,
    revocation: &WorkspaceGrantRevocation,
) -> UseResult<()> {
    if revocation.scope_id != prior.grant.scope_id
        || revocation.package_id != prior.grant.package_id
        || revocation.package_digest != prior.grant.package_digest
        || revocation.prior_revision != prior.revision
        || revocation.prior_grant_digest != prior.grant_digest
        || revocation.revision <= prior.revision
        || revocation.revoked_at_ms < prior.grant.granted_at_ms
    {
        return Err(store_error(
            "use.plugin.grant_store.revocation_invalid",
            "A workspace grant revocation does not bind the exact prior authorization state.",
        ));
    }
    Ok(())
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn path_error() -> UseError {
    store_error(
        "use.plugin.grant_store.path_invalid",
        "A workspace grant record path identity is invalid.",
    )
}

fn ownership_changed() -> UseError {
    store_error(
        "use.plugin.grant_store.ownership_changed",
        "The workspace grant changed before revocation and was preserved.",
    )
}

pub(super) fn record_error(message: impl Into<String>) -> UseError {
    store_error("use.plugin.grant_store.record_invalid", message)
}

pub(super) fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
