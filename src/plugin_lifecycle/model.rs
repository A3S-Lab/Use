use a3s_use_core::{PluginPackageId, PluginSurfaceKind, PluginSurfaceRef, UseError, UseResult};
use a3s_use_extension::SurfaceActivation;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const PLUGIN_LIFECYCLE_INTENT_SCHEMA: &str = "a3s.use.plugin-lifecycle-intent.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycleAction {
    Install,
    Upgrade,
    Enable,
    Disable,
    Uninstall,
}

impl PluginLifecycleAction {
    pub(super) fn name(self) -> &'static str {
        match self {
            Self::Install => "install",
            Self::Upgrade => "upgrade",
            Self::Enable => "enable",
            Self::Disable => "disable",
            Self::Uninstall => "uninstall",
        }
    }
}

/// Host boundary that owns one contribution kind.
///
/// The enum is lifecycle metadata, not a generic invocation protocol. Each
/// owner continues to use its typed Runtime, MCP, static projection, or
/// Knowledge contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginSurfaceHost {
    Flow,
    Knowledge,
    Mcp,
    Runtime,
    Skill,
    Ui,
}

impl PluginSurfaceHost {
    pub(super) fn for_kind(kind: PluginSurfaceKind) -> Self {
        match kind {
            PluginSurfaceKind::Flow => Self::Flow,
            PluginSurfaceKind::Tool => Self::Runtime,
            PluginSurfaceKind::Mcp => Self::Mcp,
            PluginSurfaceKind::Okf => Self::Knowledge,
            PluginSurfaceKind::Skill => Self::Skill,
            PluginSurfaceKind::Ui => Self::Ui,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleSurface {
    pub surface: PluginSurfaceRef,
    pub host: PluginSurfaceHost,
    pub activation: SurfaceActivation,
    pub required: bool,
    pub level: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginSurfaceRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycleCheckpointKind {
    PackageCommitted,
    SurfacePrepared,
    CapabilityPublished,
    CapabilityHidden,
    CallsDrained,
    SurfaceStopped,
    SurfaceRemoved,
    PackageRemoved,
}

impl PluginLifecycleCheckpointKind {
    fn name(self) -> &'static str {
        match self {
            Self::PackageCommitted => "package-committed",
            Self::SurfacePrepared => "surface-prepared",
            Self::CapabilityPublished => "capability-published",
            Self::CapabilityHidden => "capability-hidden",
            Self::CallsDrained => "calls-drained",
            Self::SurfaceStopped => "surface-stopped",
            Self::SurfaceRemoved => "surface-removed",
            Self::PackageRemoved => "package-removed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleCheckpoint {
    pub sequence: u32,
    pub kind: PluginLifecycleCheckpointKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<PluginSurfaceRef>,
    pub required: bool,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLifecycleIntentSpec {
    pub operation_id: String,
    pub plan_digest: String,
    pub scope_id: String,
    pub package_id: String,
    pub package_digest: String,
    pub manifest_digest: String,
    pub generation: u64,
    pub action: PluginLifecycleAction,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleIntent {
    pub schema: String,
    pub operation_id: String,
    pub plan_digest: String,
    pub scope_id: String,
    pub package_id: String,
    pub package_digest: String,
    pub manifest_digest: String,
    pub generation: u64,
    pub action: PluginLifecycleAction,
    pub surfaces: Vec<PluginLifecycleSurface>,
    pub checkpoints: Vec<PluginLifecycleCheckpoint>,
}

impl PluginLifecycleIntent {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_LIFECYCLE_INTENT_SCHEMA
            || !valid_machine_id(&self.operation_id)
            || !valid_machine_id(&self.scope_id)
            || PluginPackageId::parse(self.package_id.clone()).is_err()
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || self.generation == 0
            || self.surfaces.is_empty()
            || self.surfaces.len() > 256
        {
            return Err(lifecycle_error(
                "The cognitive-package lifecycle identity or surface bound is invalid.",
            ));
        }
        super::schedule::validate_surfaces(&self.surfaces)?;
        let expected = super::schedule::checkpoints(
            &self.operation_id,
            self.generation,
            self.action,
            &self.surfaces,
        )?;
        if self.checkpoints != expected {
            return Err(lifecycle_error(
                "The cognitive-package lifecycle checkpoints do not match the canonical surface schedule.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        serde_json::to_vec(self).map_err(|error| {
            lifecycle_error(format!(
                "Failed to encode the cognitive-package lifecycle intent: {error}"
            ))
        })
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }
}

pub(super) fn checkpoint_key(
    operation_id: &str,
    generation: u64,
    action: PluginLifecycleAction,
    sequence: u32,
    kind: PluginLifecycleCheckpointKind,
    surface: Option<&PluginSurfaceRef>,
) -> String {
    let surface = surface.map_or_else(
        || "package".to_string(),
        |surface| format!("{}:{}", surface_kind_name(surface.kind), surface.id),
    );
    let identity = format!(
        "{operation_id}\n{generation}\n{}\n{sequence}\n{}\n{surface}",
        action.name(),
        kind.name()
    );
    format!("sha256:{:x}", Sha256::digest(identity.as_bytes()))
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

pub(super) fn lifecycle_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_invalid", message)
}
