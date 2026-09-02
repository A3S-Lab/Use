use a3s_use_core::{InstallationPackageSelection, PluginPackageLockHost};
use serde::{Deserialize, Serialize};

use super::{
    ControlAppliedEffect, ControlEffectIntent, ControlGeneration, ControlGrantSelection,
    ControlProviderSelection,
};

/// Minimum committed package authority supplied to a non-Capability owner.
///
/// The value is projected inside the same transaction that claims the effect.
/// It deliberately carries one package incarnation rather than the complete
/// installation graph, so an owner cannot accidentally depend on unrelated
/// package authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlPackageEffectAuthority {
    pub(in crate::control_store) generation_operation_id: String,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) snapshot_digest: String,
    pub(in crate::control_store) committed_at_ms: u64,
    pub(in crate::control_store) host: PluginPackageLockHost,
    pub(in crate::control_store) package: InstallationPackageSelection,
    pub(in crate::control_store) lifecycle_generation: u64,
    pub(in crate::control_store) grant: Option<ControlGrantSelection>,
}

/// Exact reviewed Runtime selection plus its package authority.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlRuntimeEffectAuthority {
    pub(in crate::control_store) package: ControlPackageEffectAuthority,
    pub(in crate::control_store) provider_selection: ControlProviderSelection,
    /// Digest of the canonical pre-confirmation Grant proposal that produced
    /// the Runtime semantics.  A finalized Grant also carries confirmation
    /// evidence and timestamps that are deliberately excluded from Runtime
    /// semantics; binding the full finalized Grant here would create a
    /// plan-digest/confirmation-digest cycle for `Ask` authorization.
    pub(in crate::control_store) grant_proposal_digest: Option<Box<str>>,
}

/// Terminal preparation state used to materialize one target capability.
///
/// A degraded state is representable only for an explicitly optional effect;
/// required rejections never allow the Capability Index effect to be claimed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "state",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlCapabilitySurfaceState {
    Prepared {
        application: ControlAppliedEffect,
        observed_at_ms: u64,
    },
    Degraded {
        evidence_digest: String,
        error_code: String,
        observed_at_ms: u64,
    },
}

/// Latest terminal preparation for one surface in the target generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilitySurfaceAuthority {
    pub(in crate::control_store) intent: ControlEffectIntent,
    pub(in crate::control_store) state: ControlCapabilitySurfaceState,
}

/// Complete desired generation plus the exact terminal surface observations
/// needed by the Capability Index owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityEffectAuthority {
    pub(in crate::control_store) generation: ControlGeneration,
    pub(in crate::control_store) materializations: Vec<ControlCapabilitySurfaceAuthority>,
}

/// Owner-shaped committed context attached to a claimed outbox effect.
///
/// This enum makes a legacy lookup unnecessary and prevents a static owner
/// from receiving the complete installation graph or a Runtime selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) enum ControlEffectAuthority {
    CapabilityIndex(ControlCapabilityEffectAuthority),
    InvocationLeases(ControlPackageEffectAuthority),
    RuntimeProvider(ControlRuntimeEffectAuthority),
    FlowHost(ControlPackageEffectAuthority),
    KnowledgeHost(ControlPackageEffectAuthority),
    SkillHost(ControlPackageEffectAuthority),
    UiHost(ControlPackageEffectAuthority),
}
