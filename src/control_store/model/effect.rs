use a3s_use_core::{
    InstallationId, InstallationSnapshot, PluginOperationAction, PluginSurfaceRef, UseResult,
};
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::PluginLifecycleAction;

use super::{
    corruption_error, input_error, valid_machine_id, valid_sha256, ControlCapabilitySelection,
    ControlPackageLifecycle,
};

pub(in crate::control_store) const MAX_CONTROL_EFFECT_PAYLOAD_BYTES: usize = 64 * 1024;
pub(in crate::control_store) const MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlEffectKind {
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
    pub(in crate::control_store) const fn as_str(self) -> &'static str {
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

    pub(in crate::control_store) fn parse(value: &str) -> UseResult<Self> {
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
            (
                Self::Installation { .. },
                ControlEffectKind::CapabilityPublish | ControlEffectKind::CapabilityHide,
            ) => true,
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

const fn package_action_matches_kind(
    action: PluginLifecycleAction,
    kind: ControlEffectKind,
) -> bool {
    matches!(
        (action, kind),
        (
            PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade,
            ControlEffectKind::PackageCommit
        ) | (
            PluginLifecycleAction::Uninstall,
            ControlEffectKind::CallsDrain | ControlEffectKind::PackageRemove
        ) | (
            PluginLifecycleAction::Install
                | PluginLifecycleAction::Upgrade
                | PluginLifecycleAction::Enable,
            ControlEffectKind::GrantApply
        ) | (
            PluginLifecycleAction::Disable | PluginLifecycleAction::Uninstall,
            ControlEffectKind::GrantRevoke
        )
    )
}

const fn surface_action_matches_kind(
    action: PluginLifecycleAction,
    kind: ControlEffectKind,
) -> bool {
    matches!(
        (action, kind),
        (
            PluginLifecycleAction::Install
                | PluginLifecycleAction::Upgrade
                | PluginLifecycleAction::Enable,
            ControlEffectKind::SurfacePrepare | ControlEffectKind::BindingApply
        ) | (
            PluginLifecycleAction::Disable | PluginLifecycleAction::Uninstall,
            ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove
                | ControlEffectKind::BindingRemove
        )
    )
}

const fn effect_kind_matches_operation(
    kind: ControlEffectKind,
    action: PluginOperationAction,
) -> bool {
    match kind {
        ControlEffectKind::CapabilityPublish => matches!(
            action,
            PluginOperationAction::Install
                | PluginOperationAction::Upgrade
                | PluginOperationAction::Enable
        ),
        ControlEffectKind::CapabilityHide => matches!(
            action,
            PluginOperationAction::Disable | PluginOperationAction::Uninstall
        ),
        _ => true,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlEffectIntent {
    pub(in crate::control_store) sequence: u32,
    pub(in crate::control_store) idempotency_key: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) operation_action: PluginOperationAction,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) subject: ControlEffectSubject,
    pub(in crate::control_store) provider_id: String,
    pub(in crate::control_store) kind: ControlEffectKind,
    pub(in crate::control_store) required: bool,
}

impl ControlEffectIntent {
    pub(in crate::control_store) fn validate_binding(
        &self,
        installation: &InstallationId,
        plan_digest: &str,
        operation_action: PluginOperationAction,
    ) -> UseResult<()> {
        if self.installation.validate().is_err()
            || self.installation != *installation
            || !valid_sha256(&self.plan_digest)
            || self.plan_digest != plan_digest
            || self.operation_action != operation_action
            || !effect_kind_matches_operation(self.kind, operation_action)
            || !self.subject.matches_kind(self.kind)
            || !self.subject.validate_identity()
            || !valid_sha256(&self.idempotency_key)
            || !valid_machine_id(&self.provider_id)
            || self.installation_generation == 0
        {
            return Err(input_error(
                "The Control Store effect payload does not bind its reviewed operation.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        let mut bytes = Vec::new();
        let mut serializer =
            serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
        self.serialize(&mut serializer).map_err(|error| {
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

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        Ok(format!(
            "sha256:{:x}",
            Sha256::digest(self.canonical_bytes()?)
        ))
    }
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
        PluginOperationAction::Disable => is_target && action == PluginLifecycleAction::Disable,
        PluginOperationAction::Uninstall => {
            !is_target && action == PluginLifecycleAction::Uninstall
        }
    }
}
