use a3s_use_core::{
    InstallationId, InstallationSnapshot, PluginOperationAction, PluginSurfaceKind,
    PluginSurfaceRef, UseResult,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::PluginLifecycleAction;

use super::{
    corruption_error, input_error, valid_machine_id, valid_sha256, ControlCapabilitySelection,
    ControlPackageLifecycle, ControlProviderSelection,
};

pub(in crate::control_store) const CONTROL_EFFECT_INTENT_SCHEMA: &str =
    "a3s.use.control-effect-intent.v1";
pub(in crate::control_store) const MAX_CONTROL_EFFECT_PAYLOAD_BYTES: usize = 64 * 1024;
pub(in crate::control_store) const MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlEffectKind {
    SurfacePrepare,
    CapabilityCutover,
    CallsDrain,
    SurfaceStop,
    SurfaceRemove,
}

impl ControlEffectKind {
    pub(in crate::control_store) const fn as_str(self) -> &'static str {
        match self {
            Self::SurfacePrepare => "surface-prepare",
            Self::CapabilityCutover => "capability-cutover",
            Self::CallsDrain => "calls-drain",
            Self::SurfaceStop => "surface-stop",
            Self::SurfaceRemove => "surface-remove",
        }
    }

    pub(in crate::control_store) fn parse(value: &str) -> UseResult<Self> {
        match value {
            "surface-prepare" => Ok(Self::SurfacePrepare),
            "capability-cutover" => Ok(Self::CapabilityCutover),
            "calls-drain" => Ok(Self::CallsDrain),
            "surface-stop" => Ok(Self::SurfaceStop),
            "surface-remove" => Ok(Self::SurfaceRemove),
            _ => Err(corruption_error("A Control Store effect kind is invalid.")),
        }
    }
}

/// The typed port that owns one post-commit effect.
///
/// Static hosts are fixed engine composition points. Tool and MCP surfaces
/// instead bind the exact Runtime selection reviewed in the Plan. No free-form
/// provider routing string exists outside that typed Runtime variant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlEffectOwner {
    CapabilityIndex,
    InvocationLeases,
    RuntimeProvider {
        provider_id: String,
        selection_digest: String,
    },
    FlowHost,
    KnowledgeHost,
    SkillHost,
    UiHost,
}

impl ControlEffectOwner {
    pub(in crate::control_store) const fn kind_name(&self) -> &'static str {
        match self {
            Self::CapabilityIndex => "capability-index",
            Self::InvocationLeases => "invocation-leases",
            Self::RuntimeProvider { .. } => "runtime-provider",
            Self::FlowHost => "flow-host",
            Self::KnowledgeHost => "knowledge-host",
            Self::SkillHost => "skill-host",
            Self::UiHost => "ui-host",
        }
    }

    pub(in crate::control_store) fn validate_shape(
        &self,
        subject: &ControlEffectSubject,
        kind: ControlEffectKind,
    ) -> bool {
        match (self, subject, kind) {
            (
                Self::CapabilityIndex,
                ControlEffectSubject::Installation { .. },
                ControlEffectKind::CapabilityCutover,
            )
            | (
                Self::InvocationLeases,
                ControlEffectSubject::Package { .. },
                ControlEffectKind::CallsDrain,
            ) => true,
            (
                Self::RuntimeProvider {
                    provider_id,
                    selection_digest,
                },
                ControlEffectSubject::Surface { surface, .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => {
                matches!(
                    surface.kind,
                    PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
                ) && valid_machine_id(provider_id)
                    && valid_sha256(selection_digest)
            }
            (
                Self::FlowHost,
                ControlEffectSubject::Surface { surface, .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => surface.kind == PluginSurfaceKind::Flow,
            (
                Self::KnowledgeHost,
                ControlEffectSubject::Surface { surface, .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => surface.kind == PluginSurfaceKind::Okf,
            (
                Self::SkillHost,
                ControlEffectSubject::Surface { surface, .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => surface.kind == PluginSurfaceKind::Skill,
            (
                Self::UiHost,
                ControlEffectSubject::Surface { surface, .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => surface.kind == PluginSurfaceKind::Ui,
            _ => false,
        }
    }

    pub(in crate::control_store) fn matches_generation(
        &self,
        subject: &ControlEffectSubject,
        kind: ControlEffectKind,
        providers: &[ControlProviderSelection],
    ) -> bool {
        if !self.validate_shape(subject, kind) {
            return false;
        }
        let Self::RuntimeProvider {
            provider_id,
            selection_digest,
        } = self
        else {
            return true;
        };
        let ControlEffectSubject::Surface {
            package_id,
            surface,
            ..
        } = subject
        else {
            return false;
        };
        providers.iter().any(|selection| {
            selection.package_id() == package_id
                && selection.surface() == surface
                && selection.evidence.provider_id == *provider_id
                && selection.selection_digest == *selection_digest
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlEffectSubject {
    Installation {
        expected_capability_generation: u64,
        capability_generation: u64,
        descriptor_digest: String,
    },
    Package {
        package_id: String,
        lifecycle_generation: u64,
        package_digest: String,
        manifest_digest: String,
        action: PluginLifecycleAction,
    },
    Surface {
        package_id: String,
        lifecycle_generation: u64,
        package_digest: String,
        manifest_digest: String,
        action: PluginLifecycleAction,
        surface: PluginSurfaceRef,
    },
}

impl ControlEffectSubject {
    pub(in crate::control_store) const fn kind_name(&self) -> &'static str {
        match self {
            Self::Installation { .. } => "installation",
            Self::Package { .. } => "package",
            Self::Surface { .. } => "surface",
        }
    }

    pub(in crate::control_store) fn matches_kind(&self, kind: ControlEffectKind) -> bool {
        match (self, kind) {
            (Self::Installation { .. }, ControlEffectKind::CapabilityCutover) => true,
            (Self::Package { action, .. }, kind) => package_action_matches_kind(*action, kind),
            (Self::Surface { action, .. }, kind) => surface_action_matches_kind(*action, kind),
            _ => false,
        }
    }

    pub(in crate::control_store) fn validate_identity(&self) -> bool {
        match self {
            Self::Installation {
                expected_capability_generation,
                capability_generation,
                descriptor_digest,
            } => {
                expected_capability_generation.checked_add(1) == Some(*capability_generation)
                    && valid_sha256(descriptor_digest)
            }
            Self::Package {
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                ..
            }
            | Self::Surface {
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                ..
            } => {
                valid_machine_id(package_id)
                    && *lifecycle_generation > 0
                    && valid_sha256(package_digest)
                    && valid_sha256(manifest_digest)
            }
        }
    }

    pub(in crate::control_store) fn package_identity(&self) -> Option<(&str, u64)> {
        match self {
            Self::Installation { .. } => None,
            Self::Package {
                package_id,
                lifecycle_generation,
                ..
            }
            | Self::Surface {
                package_id,
                lifecycle_generation,
                ..
            } => Some((package_id, *lifecycle_generation)),
        }
    }

    pub(in crate::control_store) fn surface(&self) -> Option<&PluginSurfaceRef> {
        match self {
            Self::Surface { surface, .. } => Some(surface),
            Self::Installation { .. } | Self::Package { .. } => None,
        }
    }

    pub(in crate::control_store) fn matches_generation(
        &self,
        snapshot: &InstallationSnapshot,
        lifecycles: &[ControlPackageLifecycle],
        capability: &ControlCapabilitySelection,
        is_target: bool,
        operation_action: PluginOperationAction,
    ) -> bool {
        if !subject_action_matches_operation(self, operation_action, is_target) {
            return false;
        }
        let (package_id, lifecycle_generation, package_digest, manifest_digest, surface) =
            match self {
                Self::Installation {
                    expected_capability_generation,
                    capability_generation,
                    descriptor_digest,
                } => {
                    return is_target
                        && expected_capability_generation.checked_add(1)
                            == Some(*capability_generation)
                        && capability.generation == *capability_generation
                        && capability.descriptor_digest == *descriptor_digest;
                }
                Self::Package {
                    package_id,
                    lifecycle_generation,
                    package_digest,
                    manifest_digest,
                    ..
                } => (
                    package_id,
                    lifecycle_generation,
                    package_digest,
                    manifest_digest,
                    None,
                ),
                Self::Surface {
                    package_id,
                    lifecycle_generation,
                    package_digest,
                    manifest_digest,
                    surface,
                    ..
                } => (
                    package_id,
                    lifecycle_generation,
                    package_digest,
                    manifest_digest,
                    Some(surface),
                ),
            };
        let lifecycle_matches = lifecycles
            .binary_search_by(|lifecycle| lifecycle.package_id.as_str().cmp(package_id.as_str()))
            .ok()
            .and_then(|index| lifecycles.get(index))
            .is_some_and(|lifecycle| lifecycle.lifecycle_generation == *lifecycle_generation);
        let Some(package) = snapshot.package_selection(package_id) else {
            return false;
        };
        lifecycle_matches
            && package.package.catalog.record.package.sha256.as_deref() == Some(package_digest)
            && package
                .package
                .catalog
                .record
                .package
                .manifest_sha256
                .as_deref()
                == Some(manifest_digest)
            && surface.is_none_or(|surface| package.selected_surfaces.contains(surface))
    }
}

fn package_action_matches_kind(action: PluginLifecycleAction, kind: ControlEffectKind) -> bool {
    kind == ControlEffectKind::CallsDrain
        && matches!(
            action,
            PluginLifecycleAction::Disable | PluginLifecycleAction::Uninstall
        )
}

fn surface_action_matches_kind(action: PluginLifecycleAction, kind: ControlEffectKind) -> bool {
    matches!(
        (action, kind),
        (
            PluginLifecycleAction::Install
                | PluginLifecycleAction::Upgrade
                | PluginLifecycleAction::Enable,
            ControlEffectKind::SurfacePrepare
        ) | (
            PluginLifecycleAction::Disable,
            ControlEffectKind::SurfaceStop
        ) | (
            PluginLifecycleAction::Uninstall,
            ControlEffectKind::SurfaceRemove
        )
    )
}

fn effect_kind_matches_operation(kind: ControlEffectKind, action: PluginOperationAction) -> bool {
    match kind {
        ControlEffectKind::SurfacePrepare => matches!(
            action,
            PluginOperationAction::Install
                | PluginOperationAction::Upgrade
                | PluginOperationAction::Enable
        ),
        ControlEffectKind::CapabilityCutover => true,
        ControlEffectKind::CallsDrain => matches!(
            action,
            PluginOperationAction::Upgrade
                | PluginOperationAction::Disable
                | PluginOperationAction::Uninstall
        ),
        ControlEffectKind::SurfaceRemove => matches!(
            action,
            PluginOperationAction::Upgrade | PluginOperationAction::Uninstall
        ),
        ControlEffectKind::SurfaceStop => action == PluginOperationAction::Disable,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlEffectIntent {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) sequence: u32,
    pub(in crate::control_store) idempotency_key: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) operation_action: PluginOperationAction,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) subject: ControlEffectSubject,
    pub(in crate::control_store) owner: ControlEffectOwner,
    pub(in crate::control_store) kind: ControlEffectKind,
    pub(in crate::control_store) required: bool,
}

impl ControlEffectIntent {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::control_store) fn new(
        sequence: u32,
        installation: InstallationId,
        plan_digest: String,
        operation_action: PluginOperationAction,
        installation_generation: u64,
        subject: ControlEffectSubject,
        owner: ControlEffectOwner,
        kind: ControlEffectKind,
        required: bool,
    ) -> UseResult<Self> {
        let mut intent = Self {
            schema: CONTROL_EFFECT_INTENT_SCHEMA.to_string(),
            sequence,
            idempotency_key: String::new(),
            installation,
            plan_digest,
            operation_action,
            installation_generation,
            subject,
            owner,
            kind,
            required,
        };
        intent.idempotency_key = intent.derived_idempotency_key()?;
        intent.validate_binding(
            &intent.installation,
            &intent.plan_digest,
            intent.operation_action,
        )?;
        Ok(intent)
    }

    pub(in crate::control_store) fn validate_binding(
        &self,
        installation: &InstallationId,
        plan_digest: &str,
        operation_action: PluginOperationAction,
    ) -> UseResult<()> {
        if self.schema != CONTROL_EFFECT_INTENT_SCHEMA
            || self.installation.validate().is_err()
            || self.installation != *installation
            || !valid_sha256(&self.plan_digest)
            || self.plan_digest != plan_digest
            || self.operation_action != operation_action
            || !effect_kind_matches_operation(self.kind, operation_action)
            || !self.subject.matches_kind(self.kind)
            || !self.subject.validate_identity()
            || !self.owner.validate_shape(&self.subject, self.kind)
            || !self
                .derived_idempotency_key()
                .is_ok_and(|expected| expected == self.idempotency_key)
            || self.installation_generation == 0
        {
            return Err(input_error(
                "The Control Store effect payload does not bind its reviewed operation.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn derived_idempotency_key(&self) -> UseResult<String> {
        const DOMAIN: &[u8] = b"a3s.use.control-effect-idempotency.v1\0";
        let identity = ControlEffectIdentity {
            schema: &self.schema,
            sequence: self.sequence,
            installation: &self.installation,
            plan_digest: &self.plan_digest,
            operation_action: self.operation_action,
            installation_generation: self.installation_generation,
            subject: &self.subject,
            owner: &self.owner,
            kind: self.kind,
            required: self.required,
        };
        let bytes = canonical_effect_bytes(&identity)?;
        let mut digest = Sha256::new();
        digest.update(DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }

    pub(in crate::control_store) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        canonical_effect_bytes(self)
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlEffectIdentity<'a> {
    schema: &'a str,
    sequence: u32,
    installation: &'a InstallationId,
    plan_digest: &'a str,
    operation_action: PluginOperationAction,
    installation_generation: u64,
    subject: &'a ControlEffectSubject,
    owner: &'a ControlEffectOwner,
    kind: ControlEffectKind,
    required: bool,
}

fn canonical_effect_bytes(value: &impl Serialize) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        input_error(format!(
            "Failed to encode a canonical Control Store effect payload: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_EFFECT_PAYLOAD_BYTES {
        return Err(input_error(
            "The canonical Control Store effect payload exceeds its size bound.",
        ));
    }
    Ok(bytes)
}

fn subject_action_matches_operation(
    subject: &ControlEffectSubject,
    operation_action: PluginOperationAction,
    is_target: bool,
) -> bool {
    let action = match subject {
        ControlEffectSubject::Installation { .. } => return true,
        ControlEffectSubject::Package { action, .. }
        | ControlEffectSubject::Surface { action, .. } => *action,
    };
    match operation_action {
        PluginOperationAction::Install => is_target && action == PluginLifecycleAction::Install,
        PluginOperationAction::Upgrade => {
            (is_target
                && matches!(
                    action,
                    PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade
                ))
                || (!is_target && action == PluginLifecycleAction::Uninstall)
        }
        PluginOperationAction::Enable => is_target && action == PluginLifecycleAction::Enable,
        PluginOperationAction::Disable => !is_target && action == PluginLifecycleAction::Disable,
        PluginOperationAction::Uninstall => {
            !is_target && action == PluginLifecycleAction::Uninstall
        }
    }
}
