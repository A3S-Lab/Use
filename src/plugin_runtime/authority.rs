use std::collections::BTreeSet;
use std::fmt;

use a3s_runtime::contract::{RuntimeMount, RuntimeMountSource, SecretReference, SecretTarget};
use a3s_runtime::ProviderId;
use a3s_use_core::{
    FilesystemAccess, FilesystemPermission, FilesystemScope, PlanQualifiedSurfaceRef,
    PluginPermissionCeiling, PluginSurfaceKind, SurfacePermissionCeiling, UseError, UseResult,
};
use sha2::{Digest, Sha256};

use super::provider_selector::RuntimeProviderAssignment;

pub const RUNTIME_PLUGIN_DATA_MOUNT_ROOT: &str = "/a3s/plugin-data";
pub const RUNTIME_TEMPORARY_MOUNT_ROOT: &str = "/a3s/temporary";
pub const RUNTIME_WORKSPACE_MOUNT_ROOT: &str = "/a3s/workspace";

/// Exact host-owned Runtime resources used to enforce package authority.
///
/// The bindings are process-local composition input. They contain opaque
/// provider references, never secret values or host filesystem paths.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuntimeAuthorityBindings {
    surfaces: Vec<RuntimeSurfaceAuthorityBindings>,
}

/// Filesystem and secret bindings for one exact executable package surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSurfaceAuthorityBindings {
    surface: PlanQualifiedSurfaceRef,
    provider_id: ProviderId,
    filesystem: Vec<RuntimeFilesystemBinding>,
    secrets: Vec<RuntimeSecretBinding>,
}

/// One reviewed logical filesystem permission mapped to a bounded Runtime
/// Volume or Tmpfs mount.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeFilesystemBinding {
    permission: FilesystemPermission,
    mount: RuntimeMount,
}

/// One reviewed secret name mapped to an opaque provider reference and a typed
/// Runtime delivery target.
#[derive(Clone, PartialEq, Eq)]
pub struct RuntimeSecretBinding {
    name: String,
    secret: SecretReference,
}

impl RuntimeAuthorityBindings {
    pub fn new(mut surfaces: Vec<RuntimeSurfaceAuthorityBindings>) -> UseResult<Self> {
        surfaces.sort_by(|left, right| left.surface.cmp(&right.surface));
        if surfaces
            .windows(2)
            .any(|pair| pair[0].surface >= pair[1].surface)
        {
            return Err(binding_error(
                "Runtime authority bindings contain a duplicate executable surface.",
            ));
        }
        Ok(Self { surfaces })
    }

    pub fn surfaces(&self) -> &[RuntimeSurfaceAuthorityBindings] {
        &self.surfaces
    }

    pub(super) fn validate_against(
        &self,
        package_id: &str,
        permissions: &PluginPermissionCeiling,
    ) -> UseResult<()> {
        permissions.validate()?;
        let expected = permissions
            .surfaces
            .iter()
            .filter(|permission| {
                !permission.filesystem.is_empty() || !permission.secrets.is_empty()
            })
            .map(|permission| PlanQualifiedSurfaceRef {
                package_id: package_id.to_string(),
                surface: permission.surface.clone(),
            })
            .collect::<Vec<_>>();
        let actual = self
            .surfaces
            .iter()
            .map(|binding| binding.surface.clone())
            .collect::<Vec<_>>();
        if actual != expected {
            return Err(binding_error(
                "Runtime authority bindings must cover exactly the executable surfaces that request filesystem or secret authority.",
            ));
        }

        for (binding, permission) in
            self.surfaces
                .iter()
                .zip(permissions.surfaces.iter().filter(|permission| {
                    !permission.filesystem.is_empty() || !permission.secrets.is_empty()
                }))
        {
            binding.validate_against(permission)?;
        }
        Ok(())
    }

    pub(super) fn validate_provider_assignments(
        &self,
        assignments: &[RuntimeProviderAssignment],
    ) -> UseResult<()> {
        for binding in &self.surfaces {
            let mut matching = assignments
                .iter()
                .filter(|assignment| assignment.surface() == &binding.surface);
            let Some(assignment) = matching.next() else {
                return Err(binding_error(
                    "Every Runtime authority binding requires one exact provider assignment.",
                ));
            };
            if matching.next().is_some() || assignment.provider_id() != &binding.provider_id {
                return Err(binding_error(
                    "Runtime authority bindings cannot be reused across provider assignments.",
                ));
            }
        }
        Ok(())
    }

    pub(super) fn resources_for(
        &self,
        surface: &PlanQualifiedSurfaceRef,
    ) -> (Vec<RuntimeMount>, Vec<SecretReference>) {
        let Ok(index) = self
            .surfaces
            .binary_search_by(|binding| binding.surface.cmp(surface))
        else {
            return (Vec::new(), Vec::new());
        };
        let binding = &self.surfaces[index];
        (
            binding
                .filesystem
                .iter()
                .map(|filesystem| filesystem.mount.clone())
                .collect(),
            binding
                .secrets
                .iter()
                .map(|secret| secret.secret.clone())
                .collect(),
        )
    }
}

impl RuntimeSurfaceAuthorityBindings {
    pub fn new(
        surface: PlanQualifiedSurfaceRef,
        provider_id: impl Into<String>,
        mut filesystem: Vec<RuntimeFilesystemBinding>,
        mut secrets: Vec<RuntimeSecretBinding>,
    ) -> UseResult<Self> {
        validate_surface(&surface)?;
        let provider_id = ProviderId::parse(provider_id).map_err(|_| {
            binding_error("A Runtime surface authority binding has an invalid provider ID.")
        })?;
        filesystem.sort_by(|left, right| left.permission.cmp(&right.permission));
        secrets.sort_by(|left, right| left.name.cmp(&right.name));
        if filesystem.is_empty() && secrets.is_empty() {
            return Err(binding_error(
                "An empty Runtime surface authority binding must be omitted.",
            ));
        }
        if filesystem
            .windows(2)
            .any(|pair| pair[0].permission >= pair[1].permission)
            || secrets.windows(2).any(|pair| pair[0].name >= pair[1].name)
        {
            return Err(binding_error(
                "Runtime filesystem permissions and secret names must be unique.",
            ));
        }

        let mut mount_names = BTreeSet::new();
        let mut mount_targets = BTreeSet::new();
        let mut volume_ids = BTreeSet::new();
        for binding in &filesystem {
            if !mount_names.insert(binding.mount.name.as_str())
                || !mount_targets.insert(binding.mount.target.as_str())
            {
                return Err(binding_error(
                    "Runtime filesystem mount names and targets must be unique.",
                ));
            }
            if let RuntimeMountSource::Volume { volume_id } = &binding.mount.source {
                if !volume_ids.insert(volume_id.as_str()) {
                    return Err(binding_error(
                        "One Runtime volume cannot represent multiple logical filesystem permissions.",
                    ));
                }
            }
        }

        let mut secret_targets = BTreeSet::new();
        for binding in &secrets {
            let target = serde_json::to_string(&binding.secret.target).map_err(|error| {
                binding_error(format!(
                    "Failed to validate a Runtime secret delivery target: {error}"
                ))
            })?;
            if !secret_targets.insert(target) {
                return Err(binding_error(
                    "Runtime secret delivery targets must be unique within one surface.",
                ));
            }
        }

        Ok(Self {
            surface,
            provider_id,
            filesystem,
            secrets,
        })
    }

    pub fn surface(&self) -> &PlanQualifiedSurfaceRef {
        &self.surface
    }

    pub fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    pub fn filesystem(&self) -> &[RuntimeFilesystemBinding] {
        &self.filesystem
    }

    pub fn secrets(&self) -> &[RuntimeSecretBinding] {
        &self.secrets
    }

    fn validate_against(&self, permission: &SurfacePermissionCeiling) -> UseResult<()> {
        let filesystem = self
            .filesystem
            .iter()
            .map(|binding| binding.permission.clone())
            .collect::<Vec<_>>();
        let secrets = self
            .secrets
            .iter()
            .map(|binding| binding.name.clone())
            .collect::<Vec<_>>();
        if self.surface.surface != permission.surface
            || filesystem != permission.filesystem
            || secrets != permission.secrets
        {
            return Err(binding_error(
                "Runtime authority bindings do not exactly match the reviewed surface permissions.",
            ));
        }

        let ephemeral_limit = permission
            .resources
            .as_ref()
            .map(|resources| resources.ephemeral_storage_bytes)
            .ok_or_else(|| {
                binding_error(
                    "Filesystem and secret bindings require the executable resource ceiling.",
                )
            })?;
        let tmpfs_bytes = self.filesystem.iter().try_fold(0_u64, |total, binding| {
            let bytes = match &binding.mount.source {
                RuntimeMountSource::Tmpfs { size_bytes } => *size_bytes,
                _ => 0,
            };
            total.checked_add(bytes).ok_or_else(|| {
                binding_error("Runtime Tmpfs authority exceeds the host numeric bound.")
            })
        })?;
        if tmpfs_bytes > ephemeral_limit {
            return Err(binding_error(
                "Runtime Tmpfs authority exceeds the reviewed ephemeral-storage ceiling.",
            ));
        }
        Ok(())
    }
}

impl RuntimeFilesystemBinding {
    pub fn new(permission: FilesystemPermission, source: RuntimeMountSource) -> UseResult<Self> {
        validate_permission_path(&permission.path)?;
        match (&permission.scope, &source) {
            (
                FilesystemScope::PluginData | FilesystemScope::Workspace,
                RuntimeMountSource::Volume { volume_id },
            ) if valid_runtime_id(volume_id) => {}
            (
                FilesystemScope::Temporary,
                RuntimeMountSource::Tmpfs { size_bytes },
            ) if *size_bytes > 0 => {}
            _ => {
                return Err(binding_error(
                    "Plugin data and workspace permissions require explicit Volume bindings; temporary permissions require positive Tmpfs bindings.",
                ))
            }
        }

        let mount = RuntimeMount {
            name: mount_name(&permission),
            source,
            target: mount_target(&permission),
            read_only: permission.access == FilesystemAccess::Read,
        };
        Ok(Self { permission, mount })
    }

    pub fn permission(&self) -> &FilesystemPermission {
        &self.permission
    }

    pub fn mount(&self) -> &RuntimeMount {
        &self.mount
    }
}

impl RuntimeSecretBinding {
    pub fn new(
        name: impl Into<String>,
        reference: impl Into<String>,
        target: SecretTarget,
    ) -> UseResult<Self> {
        let name = name.into();
        let reference = reference.into();
        if !valid_permission_name(&name)
            || !valid_secret_reference(&reference)
            || !valid_secret_target(&target)
        {
            return Err(binding_error(
                "A Runtime secret binding has an invalid permission name, opaque reference, or delivery target.",
            ));
        }
        Ok(Self {
            secret: SecretReference {
                name: name.clone(),
                reference,
                target,
            },
            name,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn secret(&self) -> &SecretReference {
        &self.secret
    }
}

impl fmt::Debug for RuntimeSecretBinding {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RuntimeSecretBinding")
            .field("name", &self.name)
            .field("reference", &"<opaque-secret-reference>")
            .field("target", &self.secret.target)
            .finish()
    }
}

fn validate_surface(surface: &PlanQualifiedSurfaceRef) -> UseResult<()> {
    let package = surface.package_id.split('/').collect::<Vec<_>>();
    if surface.package_id.len() > 128
        || package.len() != 2
        || package
            .iter()
            .any(|segment| !super::model::valid_surface_segment(segment))
        || !matches!(
            surface.surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )
        || !super::model::valid_surface_segment(&surface.surface.id)
    {
        return Err(binding_error(
            "Runtime authority bindings require one valid executable package surface.",
        ));
    }
    Ok(())
}

fn mount_name(permission: &FilesystemPermission) -> String {
    let scope = scope_name(permission.scope);
    let access = match permission.access {
        FilesystemAccess::Read => "read",
        FilesystemAccess::ReadWrite => "read-write",
    };
    let identity = format!("{scope}\0{}\0{access}", permission.path);
    let digest = format!("{:x}", Sha256::digest(identity.as_bytes()));
    format!("a3s-{scope}-{}", &digest[..16])
}

fn mount_target(permission: &FilesystemPermission) -> String {
    let root = match permission.scope {
        FilesystemScope::PluginData => RUNTIME_PLUGIN_DATA_MOUNT_ROOT,
        FilesystemScope::Temporary => RUNTIME_TEMPORARY_MOUNT_ROOT,
        FilesystemScope::Workspace => RUNTIME_WORKSPACE_MOUNT_ROOT,
    };
    if permission.path == "." {
        root.to_string()
    } else {
        format!("{root}/{}", permission.path)
    }
}

fn scope_name(scope: FilesystemScope) -> &'static str {
    match scope {
        FilesystemScope::PluginData => "plugin-data",
        FilesystemScope::Temporary => "temporary",
        FilesystemScope::Workspace => "workspace",
    }
}

fn validate_permission_path(value: &str) -> UseResult<()> {
    let valid = value == "."
        || (!value.is_empty()
            && value.len() <= 1024
            && !value.starts_with('/')
            && !value.contains('\\')
            && value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'.' | b'_' | b'-')
            })
            && value
                .split('/')
                .all(|segment| !matches!(segment, "" | "." | "..")));
    if !valid {
        return Err(binding_error(
            "Runtime filesystem bindings require a portable scope-relative permission path.",
        ));
    }
    Ok(())
}

fn valid_runtime_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 255
        && !value.contains(['\0', '\r', '\n'])
        && !value.starts_with('/')
        && !value.ends_with('/')
        && !value.contains('\\')
        && !value
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
        && !matches!(
            value.as_bytes(),
            [drive, b':', b'/', ..] if drive.is_ascii_alphabetic()
        )
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.:/".contains(&byte))
}

fn valid_permission_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z' | b'0'..=b'9'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || matches!(byte, b'-' | b'.' | b'_' | b'/')
        })
        && !value
            .split('/')
            .any(|segment| matches!(segment, "" | "." | ".."))
}

fn valid_secret_reference(value: &str) -> bool {
    let Some((scheme, opaque)) = value.split_once("://") else {
        return false;
    };
    !matches!(scheme, "" | "file")
        && scheme.len() <= 64
        && matches!(scheme.as_bytes().first(), Some(b'a'..=b'z'))
        && scheme.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'+' | b'-' | b'.')
        })
        && !opaque.is_empty()
        && !opaque.starts_with('/')
        && value.len() <= 1024
        && value.bytes().all(|byte| byte.is_ascii_graphic())
}

fn valid_secret_target(target: &SecretTarget) -> bool {
    match target {
        SecretTarget::Environment { variable } => {
            let mut bytes = variable.bytes();
            bytes
                .next()
                .is_some_and(|first| first.is_ascii_alphabetic() || first == b'_')
                && variable.len() <= 255
                && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        }
        SecretTarget::File { path, mode } => {
            path.starts_with('/')
                && path.len() <= 4096
                && !path.contains('\0')
                && !path.split('/').any(|segment| segment == "..")
                && (1..=0o777).contains(mode)
        }
        SecretTarget::RegistryCredential => true,
    }
}

fn binding_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.runtime.authority_binding_invalid", message)
}
