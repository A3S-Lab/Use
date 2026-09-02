use std::collections::BTreeSet;

use a3s_use_core::{
    InstallationId, InstallationSnapshot, PluginOperationAction, PluginSurfaceKind,
    PluginWorkspaceGrant, UseError, UseResult,
};
use serde::{Deserialize, Serialize};
mod capability_publication;
mod effect;
mod effect_application;
mod effect_authority;
mod effect_state;
mod operation;
mod projection;
mod provider;

pub(super) use capability_publication::*;
pub(super) use effect::*;
pub(super) use effect_application::*;
pub(super) use effect_authority::*;
pub(super) use effect_state::*;
pub(super) use operation::*;
pub(super) use projection::*;
pub(super) use provider::*;

pub(super) const MAX_CONTROL_EFFECTS: usize = 4096;
pub(super) const MAX_CONTROL_GRANTS: usize = 4096;
const MAX_EFFECT_LEASE_MS: u64 = 5 * 60 * 1000;
pub(in crate::control_store) const MAX_EFFECT_DEFERRAL_MS: u64 = 5 * 60 * 1000;

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlGrantSelection {
    pub(super) grant: PluginWorkspaceGrant,
    pub(super) grant_digest: String,
    pub(super) receipt_revision: u64,
}

impl ControlGrantSelection {
    pub(super) fn package_id(&self) -> &str {
        &self.grant.package_id
    }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlTransition {
    pub(super) operation_id: String,
    pub(super) plan_digest: String,
    pub(super) snapshot: InstallationSnapshot,
    pub(super) package_lifecycles: Vec<ControlPackageLifecycle>,
    pub(super) grants: Vec<ControlGrantSelection>,
    pub(super) provider_selections: Vec<ControlProviderSelection>,
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
        if self.operation_id != operation.operation_id()
            || self.plan_digest != operation.plan_digest()
            || self.snapshot.installation != *installation
            || self.snapshot.generation != operation.target_generation()?
            || self.capability.generation != operation.target_capability_generation()?
            || !valid_sha256(&self.capability.descriptor_digest)
            || self.committed_at_ms < operation.reviewed_at_ms
            || self.grants.len() > MAX_CONTROL_GRANTS
            || self.provider_selections.len() > MAX_CONTROL_PROVIDER_SELECTIONS
            || self.effects.len() > MAX_CONTROL_EFFECTS
        {
            return Err(input_error(
                "The Control Store transition does not match its reviewed operation or bounds.",
            ));
        }

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

        validate_grant_selections(&self.grants, &self.snapshot)?;

        validate_provider_selections(&self.provider_selections, &self.snapshot)?;

        let mut keys = BTreeSet::new();
        let mut payload_bytes = 0_usize;
        for (index, effect) in self.effects.iter().enumerate() {
            payload_bytes = payload_bytes
                .checked_add(effect.canonical_bytes()?.len())
                .ok_or_else(|| {
                    input_error("The Control Store effect payload byte count overflowed.")
                })?;
            effect.validate_binding(installation, &self.plan_digest, operation.action())?;
            if usize::try_from(effect.sequence).ok() != Some(index)
                || !keys.insert(effect.idempotency_key.as_str())
                || (effect.installation_generation != self.snapshot.generation
                    && (operation.expected_generation == 0
                        || effect.installation_generation != operation.expected_generation))
            {
                return Err(input_error(
                    "Control Store effects must form one bounded canonical idempotent sequence.",
                ));
            }
        }
        if payload_bytes > MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES {
            return Err(input_error(
                "The Control Store effect payload sequence exceeds its total byte bound.",
            ));
        }
        Ok(())
    }

    /// Recompute caller-supplied graph, package-generation, Grant, provider,
    /// and capability fields from reviewed authority and committed history.
    /// A transition is admissible only when every projection is byte-for-byte
    /// equal.
    pub(super) fn validate_projection(
        &self,
        operation: &ReviewedControlOperation,
        prior: Option<&ControlGeneration>,
        history: &ControlProjectionHistory,
    ) -> UseResult<()> {
        let projected = operation.project_generation(prior, history, self.committed_at_ms)?;
        if self.snapshot != projected.snapshot
            || self.package_lifecycles != projected.package_lifecycles
            || self.grants != projected.grants
            || self.provider_selections != projected.provider_selections
            || self.capability != projected.capability
            || self.effects != projected.effects
        {
            return Err(input_error(
                "The Control Store transition differs from the deterministic reviewed projection.",
            ));
        }
        Ok(())
    }

    pub(super) fn validate_effect_references(
        &self,
        prior: Option<&ControlGeneration>,
    ) -> UseResult<()> {
        for effect in &self.effects {
            let generation = if effect.installation_generation == self.snapshot.generation {
                None
            } else if let Some(generation) = prior.filter(|generation| {
                generation.snapshot.generation == effect.installation_generation
            }) {
                Some(generation)
            } else {
                return Err(input_error(
                    "A Control Store effect references an unrelated installation generation.",
                ));
            };
            let is_target = generation.is_none();
            let (snapshot, lifecycles, providers, capability) = generation.map_or(
                (
                    &self.snapshot,
                    self.package_lifecycles.as_slice(),
                    self.provider_selections.as_slice(),
                    &self.capability,
                ),
                |generation| {
                    (
                        &generation.snapshot,
                        generation.package_lifecycles.as_slice(),
                        generation.provider_selections.as_slice(),
                        &generation.capability,
                    )
                },
            );
            if !effect.subject.matches_generation(
                snapshot,
                lifecycles,
                capability,
                is_target,
                effect.operation_action,
            ) || !effect
                .owner
                .matches_generation(&effect.subject, effect.kind, providers)
            {
                return Err(input_error(
                    "A Control Store effect does not bind its exact installation, package, or surface generation.",
                ));
            }
        }
        Ok(())
    }
}

pub(super) fn validate_grant_selections(
    grants: &[ControlGrantSelection],
    snapshot: &InstallationSnapshot,
) -> UseResult<()> {
    if grants.len() > MAX_CONTROL_GRANTS
        || grants
            .windows(2)
            .any(|pair| pair[0].package_id() >= pair[1].package_id())
        || grants.iter().any(|grant| {
            let Some(package) = snapshot.package_selection(grant.package_id()) else {
                return true;
            };
            grant.grant.validate().is_err()
                || grant.receipt_revision == 0
                || grant.receipt_revision > snapshot.generation.saturating_add(1)
                || grant.grant.scope_id != snapshot.installation.id
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
            "Control Store Grants must be revisioned, sorted, unique, digest-bound selected packages.",
        ));
    }
    Ok(())
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
    pub(super) provider_selections: Vec<ControlProviderSelection>,
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

pub(in crate::control_store) fn valid_error_code(value: &str) -> bool {
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
