use serde::{Deserialize, Serialize};

use crate::UseResult;

use super::validation::{
    strictly_sorted_unique, valid_dns_name, valid_http_path, valid_permission_name,
    valid_portable_scope_path, valid_segment,
};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, PluginSurfaceKind,
    PluginSurfaceRef, PLUGIN_PERMISSION_SCHEMA,
};

const PERMISSION_ERROR: &str = "use.plugin.permission_invalid";
const MAX_SURFACE_PERMISSIONS: usize = 256;
const MAX_ITEMS_PER_PERMISSION: usize = 256;
const MAX_RESOURCE_BYTES: u64 = 16 * 1024 * 1024 * 1024 * 1024;
const MAX_TASK_TIMEOUT_MS: u64 = 24 * 60 * 60 * 1000;
const MAX_CAPTURE_BYTES: u64 = 16 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPermissionCeiling {
    pub schema: String,
    pub surfaces: Vec<SurfacePermissionCeiling>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SurfacePermissionCeiling {
    pub surface: PluginSurfaceRef,
    pub native_execution: bool,
    pub child_process: bool,
    pub filesystem: Vec<FilesystemPermission>,
    pub network_egress: Vec<NetworkEgressPermission>,
    pub private_service: bool,
    pub secrets: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<ResourcePermissionCeiling>,
    pub ui_http: Vec<UiHttpPermission>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemScope {
    PluginData,
    Temporary,
    Workspace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FilesystemAccess {
    Read,
    ReadWrite,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FilesystemPermission {
    pub scope: FilesystemScope,
    pub path: String,
    pub access: FilesystemAccess,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NetworkEgressPermission {
    pub host: String,
    pub ports: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResourcePermissionCeiling {
    pub cpu_millis: u64,
    pub memory_bytes: u64,
    pub pids: u32,
    pub ephemeral_storage_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task_timeout_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_stdout_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_stderr_bytes: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HttpMethod {
    Delete,
    Get,
    Patch,
    Post,
    Put,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UiHttpPermission {
    pub tool_id: String,
    pub methods: Vec<HttpMethod>,
    pub path_prefixes: Vec<String>,
}

impl PluginPermissionCeiling {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "plugin permission ceiling",
            PERMISSION_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != PLUGIN_PERMISSION_SCHEMA
            || self.surfaces.len() > MAX_SURFACE_PERMISSIONS
            || self
                .surfaces
                .windows(2)
                .any(|pair| pair[0].surface >= pair[1].surface)
        {
            return Err(permission_error(
                "Plugin permission surfaces must use the supported schema and be sorted uniquely.",
            ));
        }
        for surface in &self.surfaces {
            surface.validate()?;
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "plugin permission ceiling", PERMISSION_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn is_within(&self, ceiling: &Self) -> UseResult<bool> {
        self.validate()?;
        ceiling.validate()?;
        Ok(self.surfaces.iter().all(|granted| {
            ceiling
                .surfaces
                .iter()
                .find(|allowed| allowed.surface == granted.surface)
                .is_some_and(|allowed| surface_is_within(granted, allowed))
        }))
    }
}

impl SurfacePermissionCeiling {
    fn validate(&self) -> UseResult<()> {
        if !valid_segment(&self.surface.id)
            || matches!(
                self.surface.kind,
                PluginSurfaceKind::Flow | PluginSurfaceKind::Okf | PluginSurfaceKind::Skill
            )
            || self.filesystem.len() > MAX_ITEMS_PER_PERMISSION
            || self.network_egress.len() > MAX_ITEMS_PER_PERMISSION
            || self.secrets.len() > MAX_ITEMS_PER_PERMISSION
            || self.ui_http.len() > MAX_ITEMS_PER_PERMISSION
            || !strictly_sorted_unique(&self.filesystem)
            || !strictly_sorted_unique(&self.network_egress)
            || !strictly_sorted_unique(&self.secrets)
            || self
                .ui_http
                .windows(2)
                .any(|pair| pair[0].tool_id >= pair[1].tool_id)
        {
            return Err(permission_error(
                "A plugin surface permission ceiling is invalid or noncanonical.",
            ));
        }

        for filesystem in &self.filesystem {
            if !valid_portable_scope_path(&filesystem.path) {
                return Err(permission_error(
                    "Filesystem permissions require portable scope-relative paths.",
                ));
            }
        }
        for egress in &self.network_egress {
            if !valid_dns_name(&egress.host)
                || egress.ports.is_empty()
                || egress.ports.len() > 16
                || egress.ports.contains(&0)
                || !strictly_sorted_unique(&egress.ports)
            {
                return Err(permission_error(
                    "Network egress permissions require exact hosts and sorted nonzero ports.",
                ));
            }
        }
        if self
            .secrets
            .iter()
            .any(|secret| !valid_permission_name(secret))
        {
            return Err(permission_error(
                "Secret permissions may contain only stable secret names.",
            ));
        }
        if let Some(resources) = &self.resources {
            resources.validate()?;
        }
        for binding in &self.ui_http {
            binding.validate()?;
        }

        match self.surface.kind {
            PluginSurfaceKind::Ui => {
                if self.native_execution
                    || self.child_process
                    || !self.filesystem.is_empty()
                    || !self.network_egress.is_empty()
                    || self.private_service
                    || !self.secrets.is_empty()
                    || self.resources.is_some()
                {
                    return Err(permission_error(
                        "UI surfaces cannot request ambient execution, filesystem, network, secret, or resource authority.",
                    ));
                }
            }
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp => {
                if !self.ui_http.is_empty() || self.resources.is_none() {
                    return Err(permission_error(
                        "Executable Tool and MCP surfaces require resource ceilings and cannot declare UI HTTP bindings.",
                    ));
                }
            }
            PluginSurfaceKind::Flow | PluginSurfaceKind::Okf | PluginSurfaceKind::Skill => {
                return Err(permission_error(
                    "Flow, OKF, and Skill surfaces cannot carry runtime permission ceilings.",
                ));
            }
        }

        if !self.native_execution
            && !self.child_process
            && self.filesystem.is_empty()
            && self.network_egress.is_empty()
            && !self.private_service
            && self.secrets.is_empty()
            && self.resources.is_none()
            && self.ui_http.is_empty()
        {
            return Err(permission_error(
                "Empty surface permission ceilings must be omitted.",
            ));
        }
        Ok(())
    }
}

impl ResourcePermissionCeiling {
    fn validate(&self) -> UseResult<()> {
        if self.cpu_millis == 0
            || self.cpu_millis > 1_000_000
            || self.memory_bytes == 0
            || self.memory_bytes > MAX_RESOURCE_BYTES
            || self.pids == 0
            || self.pids > 1_000_000
            || self.ephemeral_storage_bytes == 0
            || self.ephemeral_storage_bytes > MAX_RESOURCE_BYTES
            || self
                .task_timeout_ms
                .is_some_and(|value| value == 0 || value > MAX_TASK_TIMEOUT_MS)
            || self
                .max_stdout_bytes
                .is_some_and(|value| value == 0 || value > MAX_CAPTURE_BYTES)
            || self
                .max_stderr_bytes
                .is_some_and(|value| value == 0 || value > MAX_CAPTURE_BYTES)
        {
            return Err(permission_error(
                "Plugin resource permission ceilings are outside supported bounds.",
            ));
        }
        Ok(())
    }
}

impl UiHttpPermission {
    fn validate(&self) -> UseResult<()> {
        if !valid_segment(&self.tool_id)
            || self.methods.is_empty()
            || self.methods.len() > 16
            || !strictly_sorted_unique(&self.methods)
            || self.path_prefixes.is_empty()
            || self.path_prefixes.len() > 64
            || !strictly_sorted_unique(&self.path_prefixes)
            || self.path_prefixes.iter().any(|path| !valid_http_path(path))
        {
            return Err(permission_error(
                "A UI HTTP binding permission is invalid or noncanonical.",
            ));
        }
        Ok(())
    }
}

fn permission_error(message: impl Into<String>) -> crate::UseError {
    contract_error(PERMISSION_ERROR, message)
}

fn surface_is_within(
    granted: &SurfacePermissionCeiling,
    ceiling: &SurfacePermissionCeiling,
) -> bool {
    (!granted.native_execution || ceiling.native_execution)
        && (!granted.child_process || ceiling.child_process)
        && (!granted.private_service || ceiling.private_service)
        && granted
            .filesystem
            .iter()
            .all(|permission| filesystem_is_within(permission, &ceiling.filesystem))
        && granted
            .network_egress
            .iter()
            .all(|permission| network_is_within(permission, &ceiling.network_egress))
        && granted
            .secrets
            .iter()
            .all(|secret| ceiling.secrets.binary_search(secret).is_ok())
        && resources_are_within(granted.resources.as_ref(), ceiling.resources.as_ref())
        && granted
            .ui_http
            .iter()
            .all(|permission| ui_http_is_within(permission, &ceiling.ui_http))
}

fn filesystem_is_within(granted: &FilesystemPermission, ceiling: &[FilesystemPermission]) -> bool {
    ceiling.iter().any(|allowed| {
        granted.scope == allowed.scope
            && path_is_within(&granted.path, &allowed.path)
            && matches!(
                (granted.access, allowed.access),
                (FilesystemAccess::Read, _)
                    | (FilesystemAccess::ReadWrite, FilesystemAccess::ReadWrite)
            )
    })
}

fn network_is_within(
    granted: &NetworkEgressPermission,
    ceiling: &[NetworkEgressPermission],
) -> bool {
    granted.ports.iter().all(|port| {
        ceiling.iter().any(|allowed| {
            allowed.host == granted.host && allowed.ports.binary_search(port).is_ok()
        })
    })
}

fn resources_are_within(
    granted: Option<&ResourcePermissionCeiling>,
    ceiling: Option<&ResourcePermissionCeiling>,
) -> bool {
    match (granted, ceiling) {
        (None, _) => true,
        (Some(_), None) => false,
        (Some(granted), Some(ceiling)) => {
            granted.cpu_millis <= ceiling.cpu_millis
                && granted.memory_bytes <= ceiling.memory_bytes
                && granted.pids <= ceiling.pids
                && granted.ephemeral_storage_bytes <= ceiling.ephemeral_storage_bytes
                && optional_limit_is_within(granted.task_timeout_ms, ceiling.task_timeout_ms)
                && optional_limit_is_within(granted.max_stdout_bytes, ceiling.max_stdout_bytes)
                && optional_limit_is_within(granted.max_stderr_bytes, ceiling.max_stderr_bytes)
        }
    }
}

fn ui_http_is_within(granted: &UiHttpPermission, ceiling: &[UiHttpPermission]) -> bool {
    ceiling
        .iter()
        .find(|allowed| allowed.tool_id == granted.tool_id)
        .is_some_and(|allowed| {
            granted
                .methods
                .iter()
                .all(|method| allowed.methods.binary_search(method).is_ok())
                && granted.path_prefixes.iter().all(|path| {
                    allowed
                        .path_prefixes
                        .iter()
                        .any(|prefix| http_path_is_within(path, prefix))
                })
        })
}

fn optional_limit_is_within(granted: Option<u64>, ceiling: Option<u64>) -> bool {
    granted.is_none_or(|granted| ceiling.is_some_and(|ceiling| granted <= ceiling))
}

fn path_is_within(path: &str, parent: &str) -> bool {
    parent == "."
        || path == parent
        || path
            .strip_prefix(parent)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn http_path_is_within(path: &str, prefix: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.starts_with('/'))
}
