use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{UseError, UseResult};

mod catalog;
mod catalog_trust;
mod grant;
mod grant_changes;
mod grant_resolution;
mod manager;
mod permission;
mod plan;
mod plan_confirmation;
mod plan_package_validation;
mod plan_validation;
mod validation;

pub use catalog::{
    CatalogArchive, CatalogAvailability, CatalogMcpTransport, CatalogPackage, CatalogSurface,
    PluginCatalogRecord, PluginReleaseChannel,
};
pub use catalog_trust::{VerifiedCatalogProvenance, VerifiedPluginCatalogRecord};
pub use grant::{PluginWorkspaceGrant, WorkspaceGrantAuthority};
pub use grant_changes::{
    PlannedWorkspaceGrantChange, PluginWorkspaceGrantChangeSet, PluginWorkspaceGrantSnapshot,
    ResolvedWorkspaceGrant, ResolvedWorkspaceGrantChangeSet, WorkspaceGrantEvidence,
};
pub use grant_resolution::{
    PluginGrantConfirmation, PluginWorkspaceGrantProposal, WorkspaceGrantProposalAuthority,
};
pub use manager::{
    PluginManagerToolAnnotations, PluginManagerToolDefinition, PluginManagerToolset,
};
pub use permission::{
    FilesystemAccess, FilesystemPermission, FilesystemScope, HttpMethod, NetworkEgressPermission,
    PluginPermissionCeiling, ResourcePermissionCeiling, SurfacePermissionCeiling, UiHttpPermission,
};
pub use plan::{
    PlanActor, PlanAuthority, PlanEnforcementProfile, PlanPackageChangeKind, PlanPackageRole,
    PlanPolicyDecision, PlanQualifiedSurfaceRef, PlanScope, PlanScopeKind, PlannedOperationImpact,
    PlannedPackageState, PlannedPackageTransition, PlannedPluginRelease, PlannedProviderEvidence,
    PlannedSecretChange, PlannedSecretChangeKind, PlannedStateEvidence, PlannedSurfaceChange,
    PlannedWorkspaceImpact, PluginOperationAction, PluginOperationPlan,
    PluginOperationPlanEnvelope, PluginPlanSource, SurfaceChangeKind,
};
pub use plan_confirmation::PluginOperationConfirmation;

pub const PLUGIN_CATALOG_SCHEMA: &str = "a3s.use.plugin-catalog.v1";
pub const PLUGIN_MANAGER_TOOLSET_SCHEMA: &str = "a3s.use.plugin-manager-tools.v1";
pub const PLUGIN_OPERATION_CONFIRMATION_SCHEMA: &str = "a3s.use.plugin-operation-confirmation.v1";
pub const PLUGIN_OPERATION_PLAN_SCHEMA: &str = "a3s.use.plugin-operation-plan.v1";
pub const PLUGIN_PERMISSION_SCHEMA: &str = "a3s.use.plugin-permissions.v1";
pub const PLUGIN_GRANT_CONFIRMATION_SCHEMA: &str = "a3s.use.plugin-grant-confirmation.v1";
pub const PLUGIN_WORKSPACE_GRANT_CHANGE_SET_SCHEMA: &str =
    "a3s.use.plugin-workspace-grant-changes.v1";
pub const PLUGIN_WORKSPACE_GRANT_PROPOSAL_SCHEMA: &str =
    "a3s.use.plugin-workspace-grant-proposal.v1";
pub const PLUGIN_WORKSPACE_GRANT_SNAPSHOT_SCHEMA: &str =
    "a3s.use.plugin-workspace-grant-snapshot.v1";
pub const PLUGIN_WORKSPACE_GRANT_SCHEMA: &str = "a3s.use.plugin-workspace-grant.v1";
pub const MAX_PLUGIN_CONTRACT_BYTES: usize = 512 * 1024;
pub const MAX_PLUGIN_PLAN_ITEMS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginSurfaceKind {
    Mcp,
    Skill,
    Tool,
    Ui,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSurfaceRef {
    pub kind: PluginSurfaceKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolWorkloadClass {
    Service,
    Task,
}

fn parse_contract<T>(
    input: &[u8],
    label: &str,
    error_code: &'static str,
    validate: fn(&T) -> UseResult<()>,
) -> UseResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    if input.is_empty() || input.len() > MAX_PLUGIN_CONTRACT_BYTES {
        return Err(contract_error(
            error_code,
            format!("The {label} exceeds its input bounds."),
        ));
    }
    let contract = serde_json::from_slice(input).map_err(|error| {
        contract_error(
            error_code,
            format!(
                "Failed to decode the {label} at line {}, column {}.",
                error.line(),
                error.column()
            ),
        )
    })?;
    validate(&contract)?;
    Ok(contract)
}

fn canonical_json<T: Serialize>(
    value: &T,
    label: &str,
    error_code: &'static str,
) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        contract_error(
            error_code,
            format!("Failed to encode canonical {label} JSON: {error}"),
        )
    })?;
    if bytes.len() > MAX_PLUGIN_CONTRACT_BYTES {
        return Err(contract_error(
            error_code,
            format!("The canonical {label} exceeds its size bound."),
        ));
    }
    Ok(bytes)
}

fn canonical_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn contract_error(error_code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(error_code, message)
}
