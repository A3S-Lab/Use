use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{
    InstallationId, InstallationKind, InstallationSnapshot, PluginOperationAction, PluginPackageId,
    PluginSurfaceKind, PluginSurfaceRef, PluginWorkspaceGrant, UseError, UseResult,
};
use serde::{Deserialize, Serialize};

pub(super) const MAX_CONTROL_EFFECTS: usize = 4096;
pub(super) const MAX_CONTROL_GRANTS: usize = 4096;
pub(super) const MAX_CONTROL_BINDINGS: usize = 4096;
const MAX_EFFECT_LEASE_MS: u64 = 5 * 60 * 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ControlOperationStatus {
    Reviewed,
    EffectsPending,
    Completed,
    Rejected,
    Cancelled,
}

impl ControlOperationStatus {
    pub(super) fn parse(value: &str) -> UseResult<Self> {
        match value {
            "reviewed" => Ok(Self::Reviewed),
            "effects-pending" => Ok(Self::EffectsPending),
            "completed" => Ok(Self::Completed),
            "rejected" => Ok(Self::Rejected),
            "cancelled" => Ok(Self::Cancelled),
            _ => Err(corruption_error(
                "A Control Store operation status is invalid.",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ReviewedControlOperation {
    pub(super) operation_id: String,
    pub(super) plan_digest: String,
    pub(super) authorization_digest: String,
    pub(super) action: PluginOperationAction,
    pub(super) root_package_id: PluginPackageId,
    pub(super) expected_generation: u64,
    pub(super) expected_capability_generation: u64,
    pub(super) reviewed_at_ms: u64,
}

impl ReviewedControlOperation {
    pub(super) fn target_generation(&self) -> UseResult<u64> {
        self.expected_generation
            .checked_add(1)
            .ok_or_else(generation_exhausted)
    }

    pub(super) fn target_capability_generation(&self) -> UseResult<u64> {
        self.expected_capability_generation
            .checked_add(1)
            .ok_or_else(generation_exhausted)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.authorization_digest)
            || self.reviewed_at_ms == 0
        {
            return Err(input_error(
                "The reviewed Control Store operation identity or evidence is invalid.",
            ));
        }
        self.target_generation()?;
        self.target_capability_generation()?;
        Ok(())
    }

    pub(super) fn validate_snapshot_transition(
        &self,
        prior: Option<&InstallationSnapshot>,
        target: &InstallationSnapshot,
    ) -> UseResult<()> {
        self.validate()?;
        let prior_matches = match (self.expected_generation, prior) {
            (0, None) => true,
            (generation, Some(snapshot)) => {
                generation > 0
                    && snapshot.generation == generation
                    && snapshot.installation == target.installation
            }
            _ => false,
        };
        if !prior_matches || target.generation != self.target_generation()? {
            return Err(input_error(
                "The Control Store action does not bind consecutive installation snapshots.",
            ));
        }

        let root_package_id = self.root_package_id.as_str();
        let before_is_root = prior.is_some_and(|snapshot| {
            snapshot
                .roots
                .binary_search_by(|root| root.package_id.as_str().cmp(root_package_id))
                .is_ok()
        });
        let after_is_root = target
            .roots
            .binary_search_by(|root| root.package_id.as_str().cmp(root_package_id))
            .is_ok();
        let before_enabled = prior.and_then(|snapshot| package_enabled(snapshot, root_package_id));
        let after_enabled = package_enabled(target, root_package_id);
        let action_matches = match self.action {
            PluginOperationAction::Install => !before_is_root && after_is_root,
            PluginOperationAction::Upgrade => before_is_root && after_is_root,
            PluginOperationAction::Enable => {
                before_is_root
                    && after_is_root
                    && before_enabled == Some(false)
                    && after_enabled == Some(true)
            }
            PluginOperationAction::Disable => {
                before_is_root
                    && after_is_root
                    && before_enabled == Some(true)
                    && after_enabled == Some(false)
            }
            PluginOperationAction::Uninstall => before_is_root && !after_is_root,
        };
        if !action_matches {
            return Err(input_error(
                "The reviewed Control Store action contradicts the root package state transition.",
            ));
        }
        Ok(())
    }
}

pub(super) const fn operation_action_name(action: PluginOperationAction) -> &'static str {
    match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Enable => "enable",
        PluginOperationAction::Disable => "disable",
        PluginOperationAction::Uninstall => "uninstall",
    }
}

pub(super) fn parse_operation_action(value: &str) -> UseResult<PluginOperationAction> {
    match value {
        "install" => Ok(PluginOperationAction::Install),
        "upgrade" => Ok(PluginOperationAction::Upgrade),
        "enable" => Ok(PluginOperationAction::Enable),
        "disable" => Ok(PluginOperationAction::Disable),
        "uninstall" => Ok(PluginOperationAction::Uninstall),
        _ => Err(corruption_error(
            "A Control Store operation action is invalid.",
        )),
    }
}

fn package_enabled(snapshot: &InstallationSnapshot, package_id: &str) -> Option<bool> {
    snapshot
        .packages
        .binary_search_by(|package| package.package_id().cmp(package_id))
        .ok()
        .map(|index| snapshot.packages[index].enabled)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlGrantSelection {
    pub(super) grant: PluginWorkspaceGrant,
    pub(super) grant_digest: String,
}

impl ControlGrantSelection {
    pub(super) fn package_id(&self) -> &str {
        &self.grant.package_id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlProviderBinding {
    pub(super) package_id: String,
    pub(super) surface: PluginSurfaceRef,
    pub(super) provider_id: String,
    pub(super) binding_digest: String,
}

/// Immutable package incarnation selected by one installation generation.
///
/// This is intentionally distinct from both the installation generation and
/// `InstallationPackageSelection::state_generation`. Enable/disable advances
/// desired state without replacing the installed artifact, while upgrade may
/// select a new artifact generation in the same installation transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlPackageLifecycle {
    pub(super) package_id: String,
    pub(super) lifecycle_generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlCapabilitySelection {
    pub(super) generation: u64,
    pub(super) descriptor_digest: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ControlCapabilityStatus {
    Candidate,
    Published,
    Retired,
    Abandoned,
}

impl ControlCapabilityStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Candidate => "candidate",
            Self::Published => "published",
            Self::Retired => "retired",
            Self::Abandoned => "abandoned",
        }
    }

    pub(super) fn parse(value: &str) -> UseResult<Self> {
        match value {
            "candidate" => Ok(Self::Candidate),
            "published" => Ok(Self::Published),
            "retired" => Ok(Self::Retired),
            "abandoned" => Ok(Self::Abandoned),
            _ => Err(corruption_error(
                "A Control Store capability status is invalid.",
            )),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ControlEffectKind {
    PackageCommit,
    SurfacePrepare,
    CapabilityPublish,
    CapabilityHide,
    CallsDrain,
    SurfaceStop,
    SurfaceRemove,
    PackageRemove,
    GrantApply,
    GrantRevoke,
    BindingApply,
    BindingRemove,
}

impl ControlEffectKind {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::PackageCommit => "package-commit",
            Self::SurfacePrepare => "surface-prepare",
            Self::CapabilityPublish => "capability-publish",
            Self::CapabilityHide => "capability-hide",
            Self::CallsDrain => "calls-drain",
            Self::SurfaceStop => "surface-stop",
            Self::SurfaceRemove => "surface-remove",
            Self::PackageRemove => "package-remove",
            Self::GrantApply => "grant-apply",
            Self::GrantRevoke => "grant-revoke",
            Self::BindingApply => "binding-apply",
            Self::BindingRemove => "binding-remove",
        }
    }

    pub(super) fn parse(value: &str) -> UseResult<Self> {
        match value {
            "package-commit" => Ok(Self::PackageCommit),
            "surface-prepare" => Ok(Self::SurfacePrepare),
            "capability-publish" => Ok(Self::CapabilityPublish),
            "capability-hide" => Ok(Self::CapabilityHide),
            "calls-drain" => Ok(Self::CallsDrain),
            "surface-stop" => Ok(Self::SurfaceStop),
            "surface-remove" => Ok(Self::SurfaceRemove),
            "package-remove" => Ok(Self::PackageRemove),
            "grant-apply" => Ok(Self::GrantApply),
            "grant-revoke" => Ok(Self::GrantRevoke),
            "binding-apply" => Ok(Self::BindingApply),
            "binding-remove" => Ok(Self::BindingRemove),
            _ => Err(corruption_error("A Control Store effect kind is invalid.")),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlEffectIntent {
    pub(super) sequence: u32,
    pub(super) idempotency_key: String,
    pub(super) installation_generation: u64,
    pub(super) package_lifecycle_generation: u64,
    pub(super) package_id: String,
    pub(super) provider_id: String,
    pub(super) kind: ControlEffectKind,
    pub(super) payload_digest: String,
    pub(super) required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlTransition {
    pub(super) operation_id: String,
    pub(super) plan_digest: String,
    pub(super) snapshot: InstallationSnapshot,
    pub(super) package_lifecycles: Vec<ControlPackageLifecycle>,
    pub(super) grants: Vec<ControlGrantSelection>,
    pub(super) bindings: Vec<ControlProviderBinding>,
    pub(super) capability: ControlCapabilitySelection,
    pub(super) effects: Vec<ControlEffectIntent>,
    pub(super) committed_at_ms: u64,
}

impl ControlTransition {
    pub(super) fn validate(
        &self,
        installation: &InstallationId,
        operation: &ReviewedControlOperation,
    ) -> UseResult<()> {
        operation.validate()?;
        self.snapshot.validate()?;
        if self.operation_id != operation.operation_id
            || self.plan_digest != operation.plan_digest
            || self.snapshot.installation != *installation
            || self.snapshot.generation != operation.target_generation()?
            || self.capability.generation != operation.target_capability_generation()?
            || !valid_sha256(&self.capability.descriptor_digest)
            || self.committed_at_ms < operation.reviewed_at_ms
            || self.grants.len() > MAX_CONTROL_GRANTS
            || self.bindings.len() > MAX_CONTROL_BINDINGS
            || self.effects.len() > MAX_CONTROL_EFFECTS
        {
            return Err(input_error(
                "The Control Store transition does not match its reviewed operation or bounds.",
            ));
        }

        let packages = self
            .snapshot
            .packages
            .iter()
            .map(|selection| (selection.package_id(), selection))
            .collect::<BTreeMap<_, _>>();

        if self.package_lifecycles.len() != self.snapshot.packages.len()
            || self
                .package_lifecycles
                .windows(2)
                .any(|pair| pair[0].package_id >= pair[1].package_id)
            || self
                .package_lifecycles
                .iter()
                .zip(&self.snapshot.packages)
                .any(|(lifecycle, package)| {
                    lifecycle.package_id != package.package_id()
                        || lifecycle.lifecycle_generation == 0
                })
        {
            return Err(input_error(
                "Control Store package lifecycle generations must exactly cover the selected graph.",
            ));
        }

        if (!self.grants.is_empty() && installation.kind != InstallationKind::Workspace)
            || self
                .grants
                .windows(2)
                .any(|pair| pair[0].package_id() >= pair[1].package_id())
            || self.grants.iter().any(|grant| {
                let Some(package) = packages.get(grant.package_id()) else {
                    return true;
                };
                grant.grant.validate().is_err()
                    || grant.grant.scope_id != installation.id
                    || !grant
                        .grant
                        .descriptor_digest()
                        .is_ok_and(|digest| digest == grant.grant_digest)
                    || package.package.catalog.record.package.sha256.as_deref()
                        != Some(grant.grant.package_digest.as_str())
                    || grant
                        .grant
                        .validate_against(&package.package.catalog.record.permission_ceiling)
                        .is_err()
            })
        {
            return Err(input_error(
                "Control Store Grants must be sorted, unique, digest-bound selected packages.",
            ));
        }

        let mut prior_binding = None;
        for binding in &self.bindings {
            let key = (
                binding.package_id.as_str(),
                surface_kind_name(binding.surface.kind),
                binding.surface.id.as_str(),
                binding.provider_id.as_str(),
            );
            if prior_binding.is_some_and(|prior| prior >= key)
                || !valid_machine_id(&binding.provider_id)
                || !valid_sha256(&binding.binding_digest)
                || packages
                    .get(binding.package_id.as_str())
                    .is_none_or(|package| !package.selected_surfaces.contains(&binding.surface))
            {
                return Err(input_error(
                    "Control Store provider bindings must be sorted, unique, and bind selected surfaces.",
                ));
            }
            prior_binding = Some(key);
        }

        let mut keys = BTreeSet::new();
        for (index, effect) in self.effects.iter().enumerate() {
            if usize::try_from(effect.sequence).ok() != Some(index)
                || !keys.insert(effect.idempotency_key.as_str())
                || !valid_sha256(&effect.idempotency_key)
                || !valid_machine_id(&effect.package_id)
                || !valid_machine_id(&effect.provider_id)
                || !valid_sha256(&effect.payload_digest)
                || effect.installation_generation == 0
                || effect.package_lifecycle_generation == 0
                || (effect.installation_generation != self.snapshot.generation
                    && (operation.expected_generation == 0
                        || effect.installation_generation != operation.expected_generation))
            {
                return Err(input_error(
                    "Control Store effects must form one bounded canonical idempotent sequence.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn validate_effect_references(
        &self,
        prior: Option<&ControlGeneration>,
    ) -> UseResult<()> {
        for effect in &self.effects {
            let lifecycles = if effect.installation_generation == self.snapshot.generation {
                &self.package_lifecycles
            } else if let Some(generation) = prior.filter(|generation| {
                generation.snapshot.generation == effect.installation_generation
            }) {
                &generation.package_lifecycles
            } else {
                return Err(input_error(
                    "A Control Store effect references an unrelated installation generation.",
                ));
            };
            let matches = lifecycles
                .binary_search_by(|lifecycle| lifecycle.package_id.as_str().cmp(&effect.package_id))
                .ok()
                .and_then(|index| lifecycles.get(index))
                .is_some_and(|lifecycle| {
                    lifecycle.lifecycle_generation == effect.package_lifecycle_generation
                });
            if !matches {
                return Err(input_error(
                    "A Control Store effect does not bind the selected package lifecycle generation.",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlOperationRecord {
    pub(super) reviewed: ReviewedControlOperation,
    pub(super) status: ControlOperationStatus,
    pub(super) committed_at_ms: Option<u64>,
    pub(super) completed_at_ms: Option<u64>,
    pub(super) result_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlGeneration {
    pub(super) operation_id: String,
    pub(super) snapshot: InstallationSnapshot,
    pub(super) snapshot_digest: String,
    pub(super) package_lifecycles: Vec<ControlPackageLifecycle>,
    pub(super) grants: Vec<ControlGrantSelection>,
    pub(super) bindings: Vec<ControlProviderBinding>,
    pub(super) capability: ControlCapabilitySelection,
    pub(super) capability_status: ControlCapabilityStatus,
    pub(super) capability_published_at_ms: Option<u64>,
    pub(super) committed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlStoreAuthority {
    pub(super) generations: Vec<ControlGeneration>,
    pub(super) operations: Vec<ControlOperationRecord>,
    pub(super) effects: Vec<ControlEffectRecord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum ControlEffectStatus {
    Pending,
    Claimed,
    Applied,
    Rejected,
    Unknown,
}

impl ControlEffectStatus {
    pub(super) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }

    pub(super) fn parse(value: &str) -> UseResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "claimed" => Ok(Self::Claimed),
            "applied" => Ok(Self::Applied),
            "rejected" => Ok(Self::Rejected),
            "unknown" => Ok(Self::Unknown),
            _ => Err(corruption_error(
                "A Control Store effect status is invalid.",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlEffectRecord {
    pub(super) operation_id: String,
    pub(super) intent: ControlEffectIntent,
    pub(super) status: ControlEffectStatus,
    pub(super) attempt: u32,
    pub(super) claim_owner: Option<String>,
    pub(super) claim_token: Option<String>,
    pub(super) lease_until_ms: Option<u64>,
    pub(super) evidence_digest: Option<String>,
    pub(super) error_code: Option<String>,
    pub(super) observed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ControlEffectClaim {
    pub(super) operation_id: String,
    pub(super) worker_id: String,
    pub(super) claim_token: String,
    pub(super) now_ms: u64,
    pub(super) lease_until_ms: u64,
    pub(super) reconcile_unknown: bool,
}

impl ControlEffectClaim {
    pub(super) fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.operation_id)
            || !valid_machine_id(&self.worker_id)
            || !valid_machine_id(&self.claim_token)
            || self.now_ms == 0
            || self.lease_until_ms <= self.now_ms
            || self.lease_until_ms - self.now_ms > MAX_EFFECT_LEASE_MS
        {
            return Err(input_error("The Control Store effect claim is invalid."));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ClaimedControlEffect {
    pub(super) intent: ControlEffectIntent,
    pub(super) attempt: u32,
    pub(super) claim_token: String,
    pub(super) lease_until_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ControlEffectOutcome {
    Applied,
    Rejected,
    Unknown,
}

impl ControlEffectOutcome {
    pub(super) const fn status(self) -> ControlEffectStatus {
        match self {
            Self::Applied => ControlEffectStatus::Applied,
            Self::Rejected => ControlEffectStatus::Rejected,
            Self::Unknown => ControlEffectStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ControlEffectObservation {
    pub(super) operation_id: String,
    pub(super) idempotency_key: String,
    pub(super) claim_token: String,
    pub(super) outcome: ControlEffectOutcome,
    pub(super) evidence_digest: String,
    pub(super) error_code: Option<String>,
    pub(super) observed_at_ms: u64,
}

impl ControlEffectObservation {
    pub(super) fn validate(&self) -> UseResult<()> {
        let error_matches = match self.outcome {
            ControlEffectOutcome::Applied => self.error_code.is_none(),
            ControlEffectOutcome::Rejected | ControlEffectOutcome::Unknown => {
                self.error_code.as_deref().is_some_and(valid_error_code)
            }
        };
        if !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.idempotency_key)
            || !valid_machine_id(&self.claim_token)
            || !valid_sha256(&self.evidence_digest)
            || self.observed_at_ms == 0
            || !error_matches
        {
            return Err(input_error(
                "The Control Store effect observation is invalid.",
            ));
        }
        Ok(())
    }
}

pub(super) fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

pub(super) fn parse_surface_kind(value: &str) -> UseResult<PluginSurfaceKind> {
    match value {
        "flow" => Ok(PluginSurfaceKind::Flow),
        "mcp" => Ok(PluginSurfaceKind::Mcp),
        "okf" => Ok(PluginSurfaceKind::Okf),
        "skill" => Ok(PluginSurfaceKind::Skill),
        "tool" => Ok(PluginSurfaceKind::Tool),
        "ui" => Ok(PluginSurfaceKind::Ui),
        _ => Err(corruption_error("A Control Store surface kind is invalid.")),
    }
}

pub(super) fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

pub(super) fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

pub(super) fn input_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.input_invalid", message)
}

pub(super) fn conflict_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.conflict", message)
}

pub(super) fn corruption_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.corrupt", message)
}

fn generation_exhausted() -> UseError {
    UseError::new(
        "use.control_store.generation_exhausted",
        "The Control Store generation is exhausted.",
    )
}
