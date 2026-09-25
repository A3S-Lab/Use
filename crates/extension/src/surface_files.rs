use std::path::{Path, PathBuf};

use a3s_use_core::{
    inspect_okf_bundle_files, ExecutablePlanningSurface, McpEndpointGrantContract,
    McpReleaseDescriptor, OkfBundleFile, PlanningSurfaceActivation, PluginPlanningBundle,
    ToolReleaseDescriptor, ToolWorkloadContract as ToolReleaseWorkload, UseError, UseResult,
    MAX_RELEASE_DESCRIPTOR_BYTES,
};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncReadExt;

use super::package::{
    io_error, validate_surface_file, validate_text_asset, MAX_ACTIVITY_HTML_BYTES,
    MAX_ACTIVITY_RESOURCE_BYTES,
};
use super::{ExtensionManifest, PluginMcpLaunch, SurfaceActivation, ToolTaskSource, ToolWorkload};

const MAX_TOOL_API_CONTRACT_BYTES: u64 = 4 * 1024 * 1024;
const MAX_ENDPOINT_GRANT_BYTES: u64 = 16 * 1024;
const MAX_FLOW_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_SKILL_BYTES: u64 = 2 * 1024 * 1024;
const SURFACE_FILE_EVIDENCE_SCHEMA: &[u8] = b"a3s.use.plugin-surface-files.v1\0";

#[cfg(test)]
mod planning_binding_tests;

/// Content-addressed evidence for the immutable package files owned by one
/// named plugin surface.
///
/// Paths are hashed in portable sorted order together with their exact bytes.
/// The evidence contains no package path and can therefore be retained in a
/// lifecycle journal without disclosing local installation layout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginSurfaceFileEvidence {
    digest: String,
    file_count: u64,
    expanded_bytes: u64,
}

impl PluginSurfaceFileEvidence {
    pub fn digest(&self) -> &str {
        &self.digest
    }

    pub fn file_count(&self) -> u64 {
        self.file_count
    }

    pub fn expanded_bytes(&self) -> u64 {
        self.expanded_bytes
    }
}

pub(super) async fn validate_named_surface_files(
    manifest: &ExtensionManifest,
    canonical_root: &Path,
    package_root: &Path,
) -> UseResult<()> {
    for tool in &manifest.tools {
        match &tool.workload {
            ToolWorkload::Task(task) => {
                validate_tool_task(task, canonical_root, package_root).await?
            }
            ToolWorkload::Service(service) => {
                validate_tool_service(service, canonical_root, package_root).await?
            }
        }
    }
    for mcp in &manifest.mcp_servers {
        match &mcp.launch {
            PluginMcpLaunch::Stdio { executable, .. } => {
                validate_surface_file(
                    "MCP stdio executable",
                    canonical_root,
                    &package_root.join(executable),
                    true,
                )
                .await?;
            }
            PluginMcpLaunch::StreamableHttp { release } => {
                let path = package_root.join(release);
                validate_surface_file("MCP release descriptor", canonical_root, &path, false)
                    .await?;
                let bytes = read_bounded_file(
                    "MCP release descriptor",
                    &path,
                    MAX_RELEASE_DESCRIPTOR_BYTES as u64,
                    "use.extension.release_descriptor_invalid",
                )
                .await?;
                McpReleaseDescriptor::from_json(&bytes)
                    .map_err(|error| release_descriptor_error("MCP", &path, error))?;
            }
            PluginMcpLaunch::HostGrant { contract } => {
                validate_endpoint_grant(contract, canonical_root, package_root).await?;
            }
        }
    }
    for flow in &manifest.flows {
        validate_text_asset(
            "use.extension.flow_source_invalid",
            "A3S Flow source",
            "UTF-8 TypeScript",
            canonical_root,
            &package_root.join(&flow.source),
            MAX_FLOW_SOURCE_BYTES,
        )
        .await?;
    }
    for skill in &manifest.skills {
        validate_text_asset(
            "use.extension.skill_invalid",
            "Skill file",
            "UTF-8 Markdown",
            canonical_root,
            &package_root.join(&skill.path),
            MAX_SKILL_BYTES,
        )
        .await?;
    }
    for okf in &manifest.okf {
        validate_okf_bundle(okf, canonical_root, package_root).await?;
    }
    for ui in &manifest.ui {
        validate_ui_text_asset(
            "UI entry",
            "HTML",
            canonical_root,
            &package_root.join(&ui.entry),
            MAX_ACTIVITY_HTML_BYTES,
        )
        .await?;
        for style in &ui.styles {
            validate_ui_text_asset(
                "UI style",
                "CSS",
                canonical_root,
                &package_root.join(style),
                MAX_ACTIVITY_RESOURCE_BYTES,
            )
            .await?;
        }
        for script in &ui.scripts {
            validate_ui_text_asset(
                "UI script",
                "JavaScript",
                canonical_root,
                &package_root.join(script),
                MAX_ACTIVITY_RESOURCE_BYTES,
            )
            .await?;
        }
    }
    Ok(())
}

/// Bind the separately signed planning target to the exact digest-bound
/// package manifest and release descriptors before lifecycle admission.
pub(crate) async fn validate_planning_bundle_package_binding(
    bundle: &PluginPlanningBundle,
    manifest: &ExtensionManifest,
    package_root: &Path,
) -> UseResult<()> {
    for planning in &bundle.surfaces {
        match planning {
            ExecutablePlanningSurface::ToolTaskNative {
                id,
                activation,
                executable,
                command,
                json_output,
                timeout_ms,
            } => {
                let tool = manifest_tool(manifest, id)?;
                let ToolWorkload::Task(task) = &tool.workload else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' is not the manifest Tool Task."
                    )));
                };
                let ToolTaskSource::Executable {
                    executable: manifest_executable,
                } = &task.source
                else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' is not package-native."
                    )));
                };
                if !activation_matches(*activation, tool.activation)
                    || manifest_executable != Path::new(executable)
                    || task.command != *command
                    || task.json_output != *json_output
                    || task.timeout_ms != *timeout_ms
                {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' does not match its native manifest launcher."
                    )));
                }
            }
            ExecutablePlanningSurface::ToolTask {
                id,
                activation,
                command,
                json_output,
                timeout_ms,
                descriptor,
                ..
            } => {
                let tool = manifest_tool(manifest, id)?;
                let ToolWorkload::Task(task) = &tool.workload else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' is not the manifest Tool Task."
                    )));
                };
                let ToolTaskSource::Release { release } = &task.source else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' is not release-backed."
                    )));
                };
                if !activation_matches(*activation, tool.activation)
                    || task.command != *command
                    || task.json_output != *json_output
                    || task.timeout_ms != *timeout_ms
                    || &read_tool_release_descriptor("Tool Task", &package_root.join(release))
                        .await?
                        != descriptor
                {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' does not match its release-backed manifest launcher."
                    )));
                }
            }
            ExecutablePlanningSurface::ToolService {
                id,
                activation,
                base_path,
                descriptor,
                ..
            } => {
                let tool = manifest_tool(manifest, id)?;
                let ToolWorkload::Service(service) = &tool.workload else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' is not the manifest Tool Service."
                    )));
                };
                if !activation_matches(*activation, tool.activation)
                    || service.base_path != *base_path
                    || &read_tool_release_descriptor(
                        "Tool Service",
                        &package_root.join(&service.release),
                    )
                    .await?
                        != descriptor
                {
                    return Err(planning_package_error(format!(
                        "Planning surface 'tool/{id}' does not match its Service release."
                    )));
                }
            }
            ExecutablePlanningSurface::McpService {
                id,
                activation,
                descriptor,
                ..
            } => {
                let mcp = manifest_mcp(manifest, id)?;
                let PluginMcpLaunch::StreamableHttp { release } = &mcp.launch else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'mcp/{id}' is not the manifest Streamable HTTP service."
                    )));
                };
                let path = package_root.join(release);
                let bytes = read_bounded_file(
                    "MCP release descriptor",
                    &path,
                    MAX_RELEASE_DESCRIPTOR_BYTES as u64,
                    "use.extension.release_descriptor_invalid",
                )
                .await?;
                let manifest_descriptor = McpReleaseDescriptor::from_json(&bytes)
                    .map_err(|error| release_descriptor_error("MCP", &path, error))?;
                if !activation_matches(*activation, mcp.activation)
                    || &manifest_descriptor != descriptor
                {
                    return Err(planning_package_error(format!(
                        "Planning surface 'mcp/{id}' does not match its Service release."
                    )));
                }
            }
            ExecutablePlanningSurface::McpStdio {
                id,
                activation,
                executable,
                args,
            } => {
                let mcp = manifest_mcp(manifest, id)?;
                let PluginMcpLaunch::Stdio {
                    executable: manifest_executable,
                    args: manifest_args,
                } = &mcp.launch
                else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'mcp/{id}' is not the manifest stdio launcher."
                    )));
                };
                if !activation_matches(*activation, mcp.activation)
                    || manifest_executable != Path::new(executable)
                    || manifest_args != args
                {
                    return Err(planning_package_error(format!(
                        "Planning surface 'mcp/{id}' does not match its stdio manifest launcher."
                    )));
                }
            }
            ExecutablePlanningSurface::McpHostGrant {
                id,
                activation,
                contract_digest,
                allowed_hosts,
            } => {
                let mcp = manifest_mcp(manifest, id)?;
                let PluginMcpLaunch::HostGrant { contract } = &mcp.launch else {
                    return Err(planning_package_error(format!(
                        "Planning surface 'mcp/{id}' is not the manifest host-grant contract."
                    )));
                };
                let path = package_root.join(contract);
                let bytes = read_bounded_file(
                    "MCP endpoint grant",
                    &path,
                    MAX_ENDPOINT_GRANT_BYTES,
                    "use.plugin.mcp_endpoint_grant_invalid",
                )
                .await?;
                let parsed = McpEndpointGrantContract::from_json(&bytes).map_err(|_| {
                    planning_package_error(format!(
                        "Planning surface 'mcp/{id}' has an invalid host-grant contract."
                    ))
                })?;
                if !activation_matches(*activation, mcp.activation)
                    || &McpEndpointGrantContract::digest(&bytes)? != contract_digest
                    || &parsed.allowed_hosts != allowed_hosts
                {
                    return Err(planning_package_error(format!(
                        "Planning surface 'mcp/{id}' does not match its host-grant contract."
                    )));
                }
            }
        }
    }
    Ok(())
}

fn manifest_tool<'a>(
    manifest: &'a ExtensionManifest,
    id: &str,
) -> UseResult<&'a super::ToolSurface> {
    manifest
        .tools
        .iter()
        .find(|surface| surface.id == id)
        .ok_or_else(|| {
            planning_package_error(format!(
                "Planning surface 'tool/{id}' is absent from the package manifest."
            ))
        })
}

fn manifest_mcp<'a>(
    manifest: &'a ExtensionManifest,
    id: &str,
) -> UseResult<&'a super::PluginMcpSurface> {
    manifest
        .mcp_servers
        .iter()
        .find(|surface| surface.id == id)
        .ok_or_else(|| {
            planning_package_error(format!(
                "Planning surface 'mcp/{id}' is absent from the package manifest."
            ))
        })
}

fn activation_matches(planning: PlanningSurfaceActivation, manifest: SurfaceActivation) -> bool {
    matches!(
        (planning, manifest),
        (PlanningSurfaceActivation::Eager, SurfaceActivation::Eager)
            | (PlanningSurfaceActivation::Lazy, SurfaceActivation::Lazy)
    )
}

fn planning_package_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.planning_package_mismatch", message)
}

/// Load and revalidate the exact OKF bytes declared by one installed surface.
///
/// The returned immutable file snapshot can be passed directly to
/// `OkfKnowledgeStageRequest`, avoiding a second path-based reader at the
/// Knowledge adapter boundary.
pub async fn load_okf_bundle_files(
    surface: &super::PluginOkfSurface,
    package_root: &Path,
) -> UseResult<Vec<OkfBundleFile>> {
    surface.bundle.validate()?;
    let metadata = fs::symlink_metadata(package_root)
        .await
        .map_err(|error| io_error("inspect OKF package root", package_root, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(okf_package_error(format!(
            "OKF package root '{}' must be a real directory.",
            package_root.display()
        )));
    }
    let canonical_root = fs::canonicalize(package_root)
        .await
        .map_err(|error| io_error("resolve OKF package root", package_root, error))?;
    validate_okf_bundle(surface, &canonical_root, &canonical_root).await
}

/// Revalidate and digest the exact immutable files backing one Tool surface.
pub async fn inspect_tool_surface_files(
    surface: &super::ToolSurface,
    package_root: &Path,
) -> UseResult<PluginSurfaceFileEvidence> {
    let canonical_root = canonical_package_root(package_root, "Tool").await?;
    match &surface.workload {
        ToolWorkload::Task(task) => validate_tool_task(task, &canonical_root, package_root).await?,
        ToolWorkload::Service(service) => {
            validate_tool_service(service, &canonical_root, package_root).await?
        }
    }
    let paths = match &surface.workload {
        ToolWorkload::Task(task) => match &task.source {
            ToolTaskSource::Executable { executable } => vec![executable.clone()],
            ToolTaskSource::Release { release } => vec![release.clone()],
        },
        ToolWorkload::Service(service) => std::iter::once(service.release.clone())
            .chain(service.contract.iter().cloned())
            .collect(),
    };
    digest_surface_files(package_root, &canonical_root, paths).await
}

/// Revalidate and read the signed release descriptor backing one managed Tool
/// surface. Native executable Tasks are deliberately rejected because they do
/// not have a Runtime release plan.
pub(crate) async fn read_tool_surface_file(
    surface: &super::ToolSurface,
    package_root: &Path,
) -> UseResult<(PluginSurfaceFileEvidence, ToolReleaseDescriptor)> {
    let evidence = inspect_tool_surface_files(surface, package_root).await?;
    let release = match &surface.workload {
        super::ToolWorkload::Task(task) => match &task.source {
            super::ToolTaskSource::Release { release } => release,
            super::ToolTaskSource::Executable { .. } => {
                return Err(UseError::new(
                    "use.artifact_store.runtime_surface_invalid",
                    "A native Tool Task has no managed Runtime release descriptor.",
                ));
            }
        },
        super::ToolWorkload::Service(service) => &service.release,
    };
    let descriptor =
        read_tool_release_descriptor("Runtime Tool", &package_root.join(release)).await?;
    Ok((evidence, descriptor))
}

/// Revalidate and digest the exact immutable files backing one MCP surface.
///
/// A stdio executable remains a per-connection MCP launcher. A Streamable HTTP
/// release descriptor is only static package evidence; its process lifecycle
/// remains owned by the typed Runtime adapter.
pub async fn inspect_mcp_surface_files(
    surface: &super::PluginMcpSurface,
    package_root: &Path,
) -> UseResult<PluginSurfaceFileEvidence> {
    let canonical_root = canonical_package_root(package_root, "MCP").await?;
    let path = match &surface.launch {
        PluginMcpLaunch::Stdio { executable, .. } => {
            validate_surface_file(
                "MCP stdio executable",
                &canonical_root,
                &package_root.join(executable),
                true,
            )
            .await?;
            executable.clone()
        }
        PluginMcpLaunch::StreamableHttp { release } => {
            let path = package_root.join(release);
            validate_surface_file("MCP release descriptor", &canonical_root, &path, false).await?;
            let bytes = read_bounded_file(
                "MCP release descriptor",
                &path,
                MAX_RELEASE_DESCRIPTOR_BYTES as u64,
                "use.extension.release_descriptor_invalid",
            )
            .await?;
            McpReleaseDescriptor::from_json(&bytes)
                .map_err(|error| release_descriptor_error("MCP", &path, error))?;
            release.clone()
        }
        PluginMcpLaunch::HostGrant { contract } => {
            validate_endpoint_grant(contract, &canonical_root, package_root).await?;
            contract.clone()
        }
    };
    digest_surface_files(package_root, &canonical_root, vec![path]).await
}

async fn validate_endpoint_grant(
    contract: &Path,
    canonical_root: &Path,
    package_root: &Path,
) -> UseResult<McpEndpointGrantContract> {
    let path = package_root.join(contract);
    validate_surface_file("MCP endpoint grant", canonical_root, &path, false).await?;
    let bytes = read_bounded_file(
        "MCP endpoint grant",
        &path,
        MAX_ENDPOINT_GRANT_BYTES,
        "use.plugin.mcp_endpoint_grant_invalid",
    )
    .await?;
    McpEndpointGrantContract::from_json(&bytes).map_err(|error| {
        UseError::new(
            "use.plugin.mcp_endpoint_grant_invalid",
            format!(
                "MCP endpoint grant '{}' is invalid: {}",
                path.display(),
                error.message
            ),
        )
    })
}

/// Revalidate and read the signed release descriptor backing one managed
/// Streamable HTTP MCP surface. Stdio launchers remain native-host owned.
pub(crate) async fn read_mcp_surface_file(
    surface: &super::PluginMcpSurface,
    package_root: &Path,
) -> UseResult<(PluginSurfaceFileEvidence, McpReleaseDescriptor)> {
    let release = match &surface.launch {
        super::PluginMcpLaunch::StreamableHttp { release } => release,
        super::PluginMcpLaunch::Stdio { .. } | super::PluginMcpLaunch::HostGrant { .. } => {
            return Err(UseError::new(
                "use.artifact_store.runtime_surface_invalid",
                "A host-owned MCP launcher has no managed Runtime Service release descriptor.",
            ));
        }
    };
    let evidence = inspect_mcp_surface_files(surface, package_root).await?;
    let path = package_root.join(release);
    let bytes = read_bounded_file(
        "Runtime MCP release descriptor",
        &path,
        MAX_RELEASE_DESCRIPTOR_BYTES as u64,
        "use.extension.release_descriptor_invalid",
    )
    .await?;
    let descriptor = McpReleaseDescriptor::from_json(&bytes)
        .map_err(|error| release_descriptor_error("Runtime MCP", &path, error))?;
    Ok((evidence, descriptor))
}

/// Revalidate and digest one immutable `SKILL.md` contribution.
pub async fn inspect_skill_surface_file(
    surface: &super::PluginSkillSurface,
    package_root: &Path,
) -> UseResult<PluginSurfaceFileEvidence> {
    let canonical_root = canonical_package_root(package_root, "Skill").await?;
    validate_text_asset(
        "use.extension.skill_invalid",
        "Skill file",
        "UTF-8 Markdown",
        &canonical_root,
        &package_root.join(&surface.path),
        MAX_SKILL_BYTES,
    )
    .await?;
    digest_surface_files(package_root, &canonical_root, vec![surface.path.clone()]).await
}

/// Revalidate and digest one immutable A3S Flow TypeScript source.
///
/// This verifies package evidence only. Compilation, preflight, and execution
/// remain owned by a typed `a3s-flow` host adapter.
pub async fn inspect_flow_surface_file(
    surface: &super::PluginFlowSurface,
    package_root: &Path,
) -> UseResult<PluginSurfaceFileEvidence> {
    read_flow_surface_file(surface, package_root)
        .await
        .map(|(evidence, _)| evidence)
}

/// Read the exact bounded TypeScript bytes represented by Flow surface
/// evidence. This remains crate-private so package paths cannot cross the
/// verified Artifact Store lease boundary.
pub(crate) async fn read_flow_surface_file(
    surface: &super::PluginFlowSurface,
    package_root: &Path,
) -> UseResult<(PluginSurfaceFileEvidence, Vec<u8>)> {
    let canonical_root = canonical_package_root(package_root, "Flow").await?;
    let path = package_root.join(&surface.source);
    validate_surface_file("A3S Flow source", &canonical_root, &path, false).await?;
    let metadata = fs::symlink_metadata(&path)
        .await
        .map_err(|error| io_error("inspect A3S Flow source", &path, error))?;
    if metadata.len() == 0 || metadata.len() > MAX_FLOW_SOURCE_BYTES {
        return Err(UseError::new(
            "use.extension.flow_source_invalid",
            format!(
                "A3S Flow source '{}' must contain between 1 byte and {MAX_FLOW_SOURCE_BYTES} bytes.",
                path.display()
            ),
        ));
    }
    let capacity = usize::try_from(metadata.len()).map_err(|_| surface_evidence_limit())?;
    let mut source = Vec::with_capacity(capacity);
    let file = fs::File::open(&path)
        .await
        .map_err(|error| io_error("open A3S Flow source", &path, error))?;
    file.take(MAX_FLOW_SOURCE_BYTES.saturating_add(1))
        .read_to_end(&mut source)
        .await
        .map_err(|error| io_error("read A3S Flow source", &path, error))?;
    if source.len() as u64 != metadata.len() || source.len() as u64 > MAX_FLOW_SOURCE_BYTES {
        return Err(UseError::new(
            "use.extension.package_changed",
            format!(
                "A3S Flow source '{}' changed while it was read.",
                path.display()
            ),
        ));
    }
    std::str::from_utf8(&source).map_err(|error| {
        UseError::new(
            "use.extension.flow_source_invalid",
            format!(
                "A3S Flow source '{}' must be UTF-8 TypeScript: {error}",
                path.display()
            ),
        )
    })?;
    let portable_path = surface
        .source
        .to_str()
        .ok_or_else(|| {
            UseError::new(
                "use.extension.surface_invalid",
                "Plugin surface paths must be valid UTF-8 on every platform.",
            )
        })?
        .replace(std::path::MAIN_SEPARATOR, "/");
    let path_bytes = portable_path.as_bytes();
    let path_len = u64::try_from(path_bytes.len()).map_err(|_| surface_evidence_limit())?;
    let mut hasher = Sha256::new();
    hasher.update(SURFACE_FILE_EVIDENCE_SCHEMA);
    hasher.update(path_len.to_be_bytes());
    hasher.update(path_bytes);
    hasher.update(metadata.len().to_be_bytes());
    hasher.update(&source);
    Ok((
        PluginSurfaceFileEvidence {
            digest: format!("sha256:{:x}", hasher.finalize()),
            file_count: 1,
            expanded_bytes: metadata.len(),
        },
        source,
    ))
}

/// Revalidate and digest one immutable UI contribution and all declared
/// resources as a single surface snapshot.
pub async fn inspect_ui_surface_files(
    surface: &super::PluginUiSurface,
    package_root: &Path,
) -> UseResult<PluginSurfaceFileEvidence> {
    let canonical_root = canonical_package_root(package_root, "UI").await?;
    validate_ui_text_asset(
        "UI entry",
        "HTML",
        &canonical_root,
        &package_root.join(&surface.entry),
        MAX_ACTIVITY_HTML_BYTES,
    )
    .await?;
    for style in &surface.styles {
        validate_ui_text_asset(
            "UI style",
            "CSS",
            &canonical_root,
            &package_root.join(style),
            MAX_ACTIVITY_RESOURCE_BYTES,
        )
        .await?;
    }
    for script in &surface.scripts {
        validate_ui_text_asset(
            "UI script",
            "JavaScript",
            &canonical_root,
            &package_root.join(script),
            MAX_ACTIVITY_RESOURCE_BYTES,
        )
        .await?;
    }
    let paths = std::iter::once(surface.entry.clone())
        .chain(surface.styles.iter().cloned())
        .chain(surface.scripts.iter().cloned())
        .collect();
    digest_surface_files(package_root, &canonical_root, paths).await
}

include!("surface_files_io.rs");
