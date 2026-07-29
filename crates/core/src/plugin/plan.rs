use serde::{Deserialize, Serialize};

use crate::{UseError, UseResult};

use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, CatalogArchive,
    CatalogSurface, PluginPermissionCeiling, PluginReleaseChannel, PluginSurfaceRef,
    VerifiedCatalogProvenance,
};

pub(super) const PLAN_ERROR: &str = "use.plugin.plan_invalid";
pub(super) const MAX_PLAN_LIFETIME_MS: u64 = 60 * 60 * 1000;
pub(super) const MAX_PLAN_ITEMS: usize = 512;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPlan {
    pub schema: String,
    pub operation_id: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub action: PluginOperationAction,
    pub package_id: String,
    pub component_id: String,
    pub scope: PlanScope,
    pub packages: Vec<PlannedPackageTransition>,
    pub secret_changes: Vec<PlannedSecretChange>,
    pub providers: Vec<PlannedProviderEvidence>,
    pub workspace_impacts: Vec<PlannedWorkspaceImpact>,
    pub impact: PlannedOperationImpact,
    pub authority: PlanAuthority,
    pub state: PlannedStateEvidence,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginOperationPlanEnvelope {
    pub plan: PluginOperationPlan,
    pub plan_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginOperationAction {
    Install,
    Uninstall,
    Upgrade,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanScope {
    pub kind: PlanScopeKind,
    pub id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanScopeKind {
    User,
    Workspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedPackageTransition {
    pub package_id: String,
    pub role: PlanPackageRole,
    pub change: PlanPackageChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before: Option<PlannedPackageState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after: Option<PlannedPackageState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<PluginPlanSource>,
    pub surfaces: Vec<PlannedSurfaceChange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanPackageRole {
    Dependency,
    Root,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanPackageChangeKind {
    Add,
    Remove,
    Replace,
    Retain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedPackageState {
    pub release: PlannedPluginRelease,
    pub permissions: PluginPermissionCeiling,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum PluginPlanSource {
    Registry {
        provenance: VerifiedCatalogProvenance,
        archive: CatalogArchive,
    },
    ReleaseBundle {
        bundle_digest: String,
        package_digest: String,
    },
    LocalReviewed {
        source_digest: String,
        package_digest: String,
        unsigned: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedPluginRelease {
    pub package_id: String,
    pub version: String,
    pub channel: PluginReleaseChannel,
    pub target: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub permission_ceiling_digest: String,
    pub surfaces: Vec<CatalogSurface>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedSurfaceChange {
    pub surface: PluginSurfaceRef,
    pub change: SurfaceChangeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub before_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub after_digest: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceChangeKind {
    Add,
    Remove,
    Replace,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanQualifiedSurfaceRef {
    pub package_id: String,
    pub surface: PluginSurfaceRef,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedSecretChange {
    pub surface: PlanQualifiedSurfaceRef,
    pub secret_name: String,
    pub change: PlannedSecretChangeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlannedSecretChangeKind {
    Grant,
    Revoke,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedProviderEvidence {
    pub surface: PlanQualifiedSurfaceRef,
    pub provider_id: String,
    pub provider_build_id: String,
    pub capability_digest: String,
    pub semantics_profile_digest: String,
    pub enforcement: PlanEnforcementProfile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanEnforcementProfile {
    Container,
    NativeUnconfined,
    Sandbox,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedWorkspaceImpact {
    pub scope_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_before_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_after_digest: Option<String>,
    pub enabled_before: bool,
    pub enabled_after: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedOperationImpact {
    pub download_bytes: u64,
    pub installed_bytes_after: u64,
    pub reclaimed_bytes: u64,
    pub drain_required: bool,
    pub retained_data: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlanAuthority {
    pub actor: PlanActor,
    pub decision: PlanPolicyDecision,
    pub policy_digest: String,
    pub confirmation_required: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanActor {
    Agent,
    User,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanPolicyDecision {
    Allow,
    Ask,
    Deny,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlannedStateEvidence {
    pub state_revision: u64,
    pub capability_generation: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub receipt_digest: Option<String>,
}

impl PluginOperationPlan {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(input, "plugin operation plan", PLAN_ERROR, Self::validate)
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin operation plan", PLAN_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }
}

impl PluginOperationPlanEnvelope {
    pub fn new(plan: PluginOperationPlan) -> UseResult<Self> {
        let plan_digest = plan.descriptor_digest()?;
        Ok(Self { plan, plan_digest })
    }

    pub fn validate(&self) -> UseResult<()> {
        self.plan.validate()?;
        if self.plan.descriptor_digest()? != self.plan_digest {
            return Err(plan_error(
                "The plugin plan digest does not match its canonical content.",
            ));
        }
        Ok(())
    }

    pub fn verify_apply(
        &self,
        operation_id: &str,
        plan_digest: &str,
        now_ms: u64,
    ) -> UseResult<()> {
        self.validate()?;
        if operation_id != self.plan.operation_id || plan_digest != self.plan_digest {
            return Err(UseError::new(
                "use.plugin.plan_mismatch",
                "The plugin operation plan changed after review.",
            ));
        }
        if now_ms < self.plan.created_at_ms || now_ms >= self.plan.expires_at_ms {
            return Err(UseError::new(
                "use.plugin.plan_expired",
                "The plugin operation plan is outside its valid time window and must be resolved again.",
            ));
        }
        if self.plan.authority.decision == PlanPolicyDecision::Deny {
            return Err(UseError::new(
                "use.plugin.plan_denied",
                "Policy denies applying the plugin operation plan.",
            ));
        }
        Ok(())
    }
}

pub(super) fn plan_error(message: impl Into<String>) -> UseError {
    contract_error(PLAN_ERROR, message)
}
