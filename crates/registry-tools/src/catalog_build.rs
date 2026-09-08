//! Derive catalog-v3 records and planning bundles from package manifests
//! plus reviewed admission metadata.

use std::path::Path;

use a3s_use_core::{
    CatalogArchive, CatalogAvailability, CatalogMcpTransport, CatalogPackage, CatalogSurface,
    ExecutablePlanningSurface, PlanningSurfaceActivation, PluginCatalogRecord,
    PluginPermissionCeiling, PluginPlanningBundle, PluginReleaseChannel, PluginSurfaceKind,
    PluginSurfaceRef, ToolWorkloadClass, UseResult, PLUGIN_CATALOG_SCHEMA_V3,
    PLUGIN_PERMISSION_SCHEMA, PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use a3s_use_extension::{
    ExtensionManifest, PackageFingerprint, PluginMcpLaunch, SurfaceActivation, ToolWorkload,
};

use crate::admission::Admission;
use crate::package_build::{deterministic_package_archive, read_manifest};
use crate::tools_error;

/// Everything one admission contributes to the registry tree.
pub(crate) struct AssembledAdmission {
    pub(crate) record: PluginCatalogRecord,
    pub(crate) record_json: Vec<u8>,
    pub(crate) archive: Vec<u8>,
    pub(crate) archive_target_name: String,
    pub(crate) planning: Option<PlanningTarget>,
}

/// The separately signed `planning-v1.json` target for executable packages.
pub(crate) struct PlanningTarget {
    pub(crate) target_name: String,
    pub(crate) bytes: Vec<u8>,
}

/// Pack the package and derive its complete signed-metadata payload.
pub(crate) async fn assemble_admission(admission: &Admission) -> UseResult<AssembledAdmission> {
    let manifest = read_manifest(&admission.package_directory)?;
    if manifest.package_id != admission.package_id {
        return Err(tools_error(
            "registry_tools.admission_invalid",
            &format!(
                "Admission '{}' does not match the package manifest identity '{}'.",
                admission.package_id, manifest.package_id
            ),
        ));
    }
    let requires_use = manifest.requires_use.clone().ok_or_else(|| {
        tools_error(
            "registry_tools.package_invalid",
            &format!(
                "Package '{}' must declare requires_use before it can be admitted.",
                manifest.package_id
            ),
        )
    })?;
    let repository = admission.repository.clone().or_else(|| {
        manifest
            .repository
            .as_ref()
            .map(|repository| repository.url.clone())
    })
    .ok_or_else(|| {
        tools_error(
            "registry_tools.admission_invalid",
            &format!(
                "Admission '{}' needs a repository URL: declare one in the admission or the package manifest.",
                admission.package_id
            ),
        )
    })?;

    let archive = deterministic_package_archive(&admission.package_directory)?;
    let archive_sha256 = a3s_use_extension::sha256_hex(&archive);
    let fingerprint: PackageFingerprint =
        a3s_use_extension::package_fingerprint(&admission.package_directory).await?;
    let manifest_bytes = std::fs::read(admission.package_directory.join("a3s-use-extension.acl"))
        .map_err(|error| {
        tools_error(
            "registry_tools.package_read_failed",
            &format!(
                "Failed to re-read the manifest of '{}': {error}",
                admission.package_id
            ),
        )
    })?;
    let manifest_sha256 = a3s_use_extension::sha256_hex(&manifest_bytes);

    let archive_name = admission.archive_name.clone().unwrap_or_else(|| {
        format!(
            "{}-{}-{}.tar.gz",
            admission.package_id.replace('/', "-"),
            manifest.version,
            admission.target
        )
    });
    let archive_target_name = format!(
        "extensions/{}/{}/{}/{}/{}",
        admission.package_id,
        manifest.version,
        channel_segment(admission.channel),
        admission.target,
        archive_name
    );

    let surfaces = catalog_surfaces(&manifest)?;
    let permission_ceiling = permission_ceiling(admission, &surfaces)?;
    let permission_ceiling_digest = permission_ceiling.descriptor_digest()?;

    let planning = planning_target(
        admission,
        &manifest,
        &archive_sha256,
        &fingerprint.sha256,
        &manifest_sha256,
        &permission_ceiling_digest,
        &surfaces,
    )?;

    let has_executable = surfaces.iter().any(|surface| {
        matches!(
            surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )
    });
    if has_executable != planning.is_some() {
        return Err(tools_error(
            "registry_tools.planning_invalid",
            &format!(
                "Package '{}' carries executable surfaces exactly when it carries a planning target.",
                admission.package_id
            ),
        ));
    }

    let record = PluginCatalogRecord {
        schema: PLUGIN_CATALOG_SCHEMA_V3.to_string(),
        package_id: manifest.package_id.clone(),
        display_name: admission.display_name.clone(),
        description: admission.description.clone(),
        publisher: admission
            .package_id
            .split('/')
            .next()
            .unwrap_or_default()
            .to_string(),
        keywords: sorted_unique(admission.keywords.clone()),
        categories: sorted_unique(admission.categories.clone()),
        version: manifest.version.clone(),
        channel: admission.channel,
        requires_use,
        dependencies: manifest.dependencies.clone(),
        target: admission.target.clone(),
        surfaces,
        permission_ceiling_digest,
        permission_ceiling,
        planning: planning.as_ref().map(|planning| {
            use a3s_use_core::CatalogPlanningTarget;
            CatalogPlanningTarget {
                target_name: planning.target_name.clone(),
                length: planning.bytes.len() as u64,
                sha256: format!("sha256:{}", a3s_use_extension::sha256_hex(&planning.bytes)),
            }
        }),
        archive: CatalogArchive {
            target_name: archive_target_name.clone(),
            length: archive.len() as u64,
            sha256: format!("sha256:{archive_sha256}"),
        },
        package: CatalogPackage {
            expanded_bytes: fingerprint.byte_count,
            file_count: fingerprint.file_count,
            sha256: Some(format!("sha256:{}", fingerprint.sha256)),
            manifest_sha256: Some(format!("sha256:{manifest_sha256}")),
        },
        license: admission.license.clone(),
        repository,
        availability: CatalogAvailability::Available,
    };
    record.validate()?;
    let record_json = record.canonical_bytes()?;
    Ok(AssembledAdmission {
        record,
        record_json,
        archive,
        archive_target_name,
        planning,
    })
}

fn channel_segment(channel: PluginReleaseChannel) -> &'static str {
    match channel {
        PluginReleaseChannel::Stable => "stable",
        PluginReleaseChannel::Beta => "beta",
        PluginReleaseChannel::Nightly => "nightly",
    }
}

fn sorted_unique(mut values: Vec<String>) -> Vec<String> {
    values.sort();
    values.dedup();
    values
}

fn catalog_surfaces(manifest: &ExtensionManifest) -> UseResult<Vec<CatalogSurface>> {
    let mut surfaces = Vec::new();
    for tool in &manifest.tools {
        surfaces.push(CatalogSurface {
            kind: PluginSurfaceKind::Tool,
            id: tool.id.clone(),
            optional: tool.optional,
            workload: Some(match tool.workload {
                ToolWorkload::Task(_) => ToolWorkloadClass::Task,
                ToolWorkload::Service(_) => ToolWorkloadClass::Service,
            }),
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: Vec::new(),
        });
    }
    for mcp in &manifest.mcp_servers {
        surfaces.push(CatalogSurface {
            kind: PluginSurfaceKind::Mcp,
            id: mcp.id.clone(),
            optional: mcp.optional,
            workload: None,
            mcp_transport: Some(match mcp.launch {
                PluginMcpLaunch::Stdio { .. } => CatalogMcpTransport::Stdio,
                PluginMcpLaunch::StreamableHttp { .. } => CatalogMcpTransport::StreamableHttp,
            }),
            mcp_tool_count: None,
            okf_bundle: None,
            requires: Vec::new(),
        });
    }
    for okf in &manifest.okf {
        surfaces.push(CatalogSurface {
            kind: PluginSurfaceKind::Okf,
            id: okf.id.clone(),
            optional: okf.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: Some(okf.bundle.clone()),
            requires: Vec::new(),
        });
    }
    for flow in &manifest.flows {
        surfaces.push(CatalogSurface {
            kind: PluginSurfaceKind::Flow,
            id: flow.id.clone(),
            optional: flow.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: Vec::new(),
        });
    }
    for skill in &manifest.skills {
        let mut requires = Vec::new();
        for dependency in &skill.requires_tools {
            requires.push(PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: dependency.clone(),
            });
        }
        for dependency in &skill.requires_mcp {
            requires.push(PluginSurfaceRef {
                kind: PluginSurfaceKind::Mcp,
                id: dependency.clone(),
            });
        }
        for dependency in &skill.requires_okf {
            requires.push(PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: dependency.clone(),
            });
        }
        for dependency in &skill.requires_flows {
            requires.push(PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: dependency.clone(),
            });
        }
        requires.sort();
        surfaces.push(CatalogSurface {
            kind: PluginSurfaceKind::Skill,
            id: skill.id.clone(),
            optional: skill.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires,
        });
    }
    for ui in &manifest.ui {
        surfaces.push(CatalogSurface {
            kind: PluginSurfaceKind::Ui,
            id: ui.id.clone(),
            optional: ui.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: Vec::new(),
        });
    }
    surfaces.sort_by(|left, right| {
        left.kind
            .cmp(&right.kind)
            .then_with(|| left.id.cmp(&right.id))
    });
    Ok(surfaces)
}

fn permission_ceiling(
    admission: &Admission,
    surfaces: &[CatalogSurface],
) -> UseResult<PluginPermissionCeiling> {
    let executable = surfaces.iter().any(|surface| {
        matches!(
            surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )
    });
    if !executable {
        return Ok(PluginPermissionCeiling {
            schema: PLUGIN_PERMISSION_SCHEMA.to_string(),
            surfaces: Vec::new(),
        });
    }
    let Some(path) = &admission.permission_ceiling else {
        return Err(tools_error(
            "registry_tools.admission_invalid",
            &format!(
                "Admission '{}' has executable surfaces and must declare permission_ceiling.",
                admission.package_id
            ),
        ));
    };
    let bytes = std::fs::read(path).map_err(|error| {
        tools_error(
            "registry_tools.admission_read_failed",
            &format!(
                "Failed to read the permission ceiling '{:?}': {error}",
                path
            ),
        )
    })?;
    let ceiling: PluginPermissionCeiling = serde_json::from_slice(&bytes).map_err(|error| {
        tools_error(
            "registry_tools.admission_invalid",
            &format!("The permission ceiling is not valid JSON: {error}"),
        )
    })?;
    ceiling.validate()?;
    Ok(ceiling)
}

fn planning_target(
    admission: &Admission,
    manifest: &ExtensionManifest,
    archive_sha256: &str,
    package_sha256: &str,
    manifest_digest: &str,
    permission_ceiling_digest: &str,
    surfaces: &[CatalogSurface],
) -> UseResult<Option<PlanningTarget>> {
    let has_executable = surfaces.iter().any(|surface| {
        matches!(
            surface.kind,
            PluginSurfaceKind::Tool | PluginSurfaceKind::Mcp
        )
    });
    if !has_executable {
        return Ok(None);
    }
    let mut planning_surfaces = Vec::new();
    for tool in &manifest.tools {
        let activation = planning_activation(tool.activation);
        planning_surfaces.push(match &tool.workload {
            ToolWorkload::Task(task) => match &task.source {
                a3s_use_extension::ToolTaskSource::Executable { executable } => {
                    ExecutablePlanningSurface::ToolTaskNative {
                        id: tool.id.clone(),
                        activation,
                        executable: portable_path(executable)?,
                        command: task.command.clone(),
                        json_output: task.json_output,
                        timeout_ms: task.timeout_ms,
                    }
                }
                a3s_use_extension::ToolTaskSource::Release { .. } => {
                    return Err(unsupported_release_surface(
                        &admission.package_id,
                        "release-backed Tool Task",
                    ));
                }
            },
            ToolWorkload::Service(_) => {
                return Err(unsupported_release_surface(
                    &admission.package_id,
                    "Tool Service",
                ));
            }
        });
    }
    for mcp in &manifest.mcp_servers {
        let activation = planning_activation(mcp.activation);
        planning_surfaces.push(match &mcp.launch {
            PluginMcpLaunch::Stdio { executable, args } => ExecutablePlanningSurface::McpStdio {
                id: mcp.id.clone(),
                activation,
                executable: portable_path(executable)?,
                args: args.clone(),
            },
            PluginMcpLaunch::StreamableHttp { .. } => {
                return Err(unsupported_release_surface(
                    &admission.package_id,
                    "Streamable HTTP MCP",
                ));
            }
        });
    }
    let target_name = format!(
        "extensions/{}/{}/{}/{}/planning-v1.json",
        admission.package_id,
        manifest.version,
        channel_segment(admission.channel),
        admission.target
    );
    let bundle = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_string(),
        package_id: admission.package_id.clone(),
        version: manifest.version.clone(),
        channel: admission.channel,
        target: admission.target.clone(),
        archive_sha256: format!("sha256:{archive_sha256}"),
        package_sha256: format!("sha256:{package_sha256}"),
        manifest_sha256: format!("sha256:{manifest_digest}"),
        permission_ceiling_digest: permission_ceiling_digest.to_string(),
        surfaces: planning_surfaces,
    };
    Ok(Some(PlanningTarget {
        target_name,
        bytes: bundle.canonical_bytes()?,
    }))
}

fn unsupported_release_surface(package_id: &str, kind: &str) -> a3s_use_core::UseError {
    tools_error(
        "registry_tools.planning_invalid",
        &format!(
            "Package '{package_id}' declares a {kind} surface. Release-backed surfaces need \
             descriptor assembly that registry-tools does not implement yet; admit the package \
             through package-local executable surfaces first."
        ),
    )
    .with_suggestion(
        "Convert the surface to a package-local executable, or extend registry-tools with \
         release-descriptor assembly before admitting this package.",
    )
}

fn planning_activation(activation: SurfaceActivation) -> PlanningSurfaceActivation {
    match activation {
        SurfaceActivation::Eager => PlanningSurfaceActivation::Eager,
        SurfaceActivation::Lazy => PlanningSurfaceActivation::Lazy,
    }
}

fn portable_path(path: &Path) -> UseResult<String> {
    let segments = path
        .iter()
        .map(|segment| segment.to_str().unwrap_or_default())
        .collect::<Vec<_>>()
        .join("/");
    if segments.starts_with('/') || segments.contains("..") || segments.contains('\\') {
        return Err(tools_error(
            "registry_tools.planning_invalid",
            &format!("Executable path '{segments}' must be a relative package-local path."),
        ));
    }
    Ok(segments)
}
