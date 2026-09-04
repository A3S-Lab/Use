use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{UseError, UseResult};

mod capability_consumer;
mod capability_gateway;
mod catalog;
mod catalog_plan;
mod catalog_selection;
mod catalog_trust;
mod grant;
mod grant_changes;
mod grant_resolution;
mod host;
mod host_cancellation;
mod host_enablement_plan;
mod host_observation;
mod host_operation;
mod host_operation_observation;
mod host_plan;
mod identity;
mod installation;
mod installation_snapshot;
mod installed_plan;
mod manager;
mod manager_input;
mod package_dependency;
mod package_lock;
mod package_resolution;
mod permission;
mod plan;
mod plan_confirmation;
mod plan_draft;
mod plan_package_validation;
mod plan_validation;
mod planning_bundle;
mod resolved_grant_changes;
pub(crate) mod validation;

pub use capability_consumer::{
    CapabilityConsumerExtension, CapabilityConsumerKind, CapabilityConsumerNegotiation,
    CapabilityConsumerProfile, CAPABILITY_CONSUMER_NEGOTIATION_SCHEMA_V1,
    CAPABILITY_CONSUMER_PROFILE_SCHEMA_V1, MAX_CAPABILITY_CONSUMER_EXTENSIONS,
};
pub use capability_gateway::{
    ArtifactRef, CapabilityDescriptionProof, CapabilityDescriptor, CapabilityDescriptorKind,
    CapabilityGatewayCatalog, CapabilityMcpTransport, CapabilityPublicationEvidence,
    CapabilityToolAnnotations, EndpointRef, InvocationRef, CAPABILITY_DESCRIPTION_PROOF_SCHEMA_V1,
    CAPABILITY_DESCRIPTOR_SCHEMA_V1, CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1,
};
pub use catalog::{
    CatalogArchive, CatalogAvailability, CatalogMcpTransport, CatalogPackage,
    CatalogPlanningTarget, CatalogSurface, PluginCatalogRecord, PluginReleaseChannel,
};
pub use catalog_trust::{VerifiedCatalogProvenance, VerifiedPluginCatalogRecord};
pub use grant::{PluginWorkspaceGrant, WorkspaceGrantAuthority};
pub use grant_changes::{
    PlannedWorkspaceGrantChange, PluginWorkspaceGrantChangeSet, PluginWorkspaceGrantSnapshot,
    WorkspaceGrantEvidence,
};
pub use grant_resolution::{
    PluginGrantConfirmation, PluginWorkspaceGrantProposal, WorkspaceGrantProposalAuthority,
};
pub use host::{
    PluginHostCapabilities, PluginHostManager, PluginManagedScope,
    PLUGIN_HOST_CAPABILITIES_SCHEMA_V6, PLUGIN_HOST_PROTOCOL_LEVEL_V6,
    PLUGIN_MANAGED_SCOPE_SCHEMA_V2,
};
pub use host_cancellation::{
    PluginHostCancelRequest, PluginHostCancelResult, PluginHostCancellationStatus,
    PLUGIN_HOST_CANCEL_REQUEST_SCHEMA, PLUGIN_HOST_CANCEL_RESULT_SCHEMA,
};
pub use host_enablement_plan::{
    PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
    PluginHostEnablementPlanStatus, PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA,
};
pub use host_observation::{
    PluginDesiredState, PluginHostObservationRequest, PluginHostObservationResult,
    PluginHostObservationStatus, PluginHostPackageState, PluginHostUnavailableReason,
    PluginObservedState, PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA,
};
pub use host_operation::{
    PluginHostApplyRequest, PluginHostApplyResult, PLUGIN_HOST_APPLY_REQUEST_SCHEMA,
    PLUGIN_HOST_APPLY_RESULT_SCHEMA,
};
pub use host_operation_observation::{
    PluginHostOperationCancellability, PluginHostOperationObservationRequest,
    PluginHostOperationObservationResult, PluginHostOperationPhase, PluginHostOperationProgress,
    PluginHostOperationStatus, PluginHostOperationWatchRequest,
    MAX_PLUGIN_HOST_OPERATION_WATCH_TIMEOUT_MS, PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA,
    PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA, PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA,
};
pub use host_plan::{
    PluginHostPlanRequest, PluginHostPlanResult, PLUGIN_HOST_PLAN_REQUEST_SCHEMA,
    PLUGIN_HOST_PLAN_RESULT_SCHEMA,
};
pub use identity::PluginPackageId;
pub use installation::{InstallationId, InstallationKind, PlanScope, PlanScopeKind};
pub use installation_snapshot::{
    InstallationPackageSelection, InstallationRootSelection, InstallationSnapshot,
    INSTALLATION_SNAPSHOT_SCHEMA, MAX_INSTALLATION_PACKAGES, MAX_INSTALLATION_ROOTS,
    MAX_INSTALLATION_SNAPSHOT_BYTES,
};
pub use installed_plan::InstalledPluginPlanEvidence;
pub use manager::{
    PluginManagerToolAnnotations, PluginManagerToolDefinition, PluginManagerToolset,
};
pub use manager_input::{
    PluginManagerApplyPlanInput, PluginManagerInspectInput, PluginManagerInstallPlanInput,
    PluginManagerListInstalledInput, PluginManagerPackageScopeInput, PluginManagerSearchInput,
    PluginManagerUpgradePlanInput,
};
pub use package_dependency::{PluginPackageDependency, MAX_PLUGIN_PACKAGE_DEPENDENCIES};
pub use package_lock::{
    LockedPluginPackage, LockedPluginPackageDependency, PluginPackageLock, PluginPackageLockHost,
    PLUGIN_PACKAGE_LOCK_SCHEMA,
};
pub use package_resolution::{PluginPackageResolver, MAX_PLUGIN_RESOLUTION_CANDIDATES};
pub use permission::{
    FilesystemAccess, FilesystemPermission, FilesystemScope, HttpMethod, NetworkEgressPermission,
    PluginPermissionCeiling, ResourcePermissionCeiling, SurfacePermissionCeiling, UiHttpPermission,
};
pub use plan::{
    PlanActor, PlanAuthority, PlanEnforcementProfile, PlanPackageChangeKind, PlanPackageRole,
    PlanPolicyDecision, PlanQualifiedSurfaceRef, PlannedOkfSurfaceChange, PlannedOperationImpact,
    PlannedPackageState, PlannedPackageTransition, PlannedPluginRelease, PlannedProviderEvidence,
    PlannedSecretChange, PlannedSecretChangeKind, PlannedStateEvidence, PlannedSurfaceChange,
    PlannedWorkspaceImpact, PluginOperationAction, PluginOperationPlan,
    PluginOperationPlanEnvelope, PluginPlanSource, SurfaceChangeKind,
};
pub use plan_confirmation::PluginOperationConfirmation;
pub use plan_draft::{PluginOperationPlanBinding, PluginOperationPlanDraft};
pub use planning_bundle::{
    ExecutablePlanningSurface, PlanningArtifactRef, PlanningSurfaceActivation, PluginPlanningBundle,
};
pub use resolved_grant_changes::{ResolvedWorkspaceGrant, ResolvedWorkspaceGrantChangeSet};

pub const PLUGIN_CATALOG_SCHEMA_V3: &str = "a3s.use.plugin-catalog.v3";
pub const INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA: &str = "a3s.use.installed-plugin-plan-evidence.v1";
pub const PLUGIN_MANAGER_TOOLSET_SCHEMA_V4: &str = "a3s.use.plugin-manager-tools.v4";
pub const PLUGIN_OPERATION_CONFIRMATION_SCHEMA: &str = "a3s.use.plugin-operation-confirmation.v1";
/// Current draft schema for every package lifecycle operation.
pub const PLUGIN_OPERATION_PLAN_DRAFT_SCHEMA_V3: &str = "a3s.use.plugin-operation-plan-draft.v3";
/// Current plan schema for install, upgrade, uninstall, enable, and disable.
pub const PLUGIN_OPERATION_PLAN_SCHEMA_V4: &str = "a3s.use.plugin-operation-plan.v4";
pub const PLUGIN_PLANNING_BUNDLE_SCHEMA: &str = "a3s.use.plugin-planning-bundle.v1";
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
    Flow,
    Mcp,
    Okf,
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
