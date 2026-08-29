use std::collections::BTreeSet;
use std::path::{Component, Path};

use a3s_acl::{Block, Value};
use a3s_use_core::{
    PluginPackageDependency, PluginSurfaceKind, PluginSurfaceRef, RiskClass, UseError, UseResult,
};
use serde::{Deserialize, Serialize};

#[cfg(test)]
extern crate self as a3s_use_extension;

#[cfg(all(test, any(unix, windows)))]
mod test_filesystem;

mod artifact_store;
mod atomic_file;
mod digest;
mod generation_lease;
mod package;
#[cfg(test)]
mod package_tests;
mod paths;
mod plugin_manifest;
#[cfg(test)]
mod plugin_manifest_tests;
mod registry;
mod registry_io;
mod registry_sources;
mod release_bundle;
mod remote;
mod source;
mod state_maintenance;
mod surface_files;
mod workspace_grant;
mod workspace_grant_io;
mod workspace_grant_lifecycle;
#[cfg(test)]
mod workspace_grant_lifecycle_fault_matrix;
mod workspace_grant_operation;
mod workspace_grant_operation_io;
mod workspace_grant_snapshot;
#[cfg(test)]
mod workspace_grant_tests;

pub use artifact_store::{ArtifactCollectionGuard, ArtifactReferenceAdmission, ArtifactStore};
#[doc(hidden)]
pub use atomic_file::{
    persist_named_temporary_noclobber_blocking, persist_temporary_noclobber_blocking,
    persist_temporary_replace_blocking, rename_path_with_windows_retry_blocking,
};
pub use paths::{ExtensionPaths, UsePaths};
pub use plugin_manifest::{
    PluginFlowEngine, PluginFlowRuntime, PluginFlowSurface, PluginMcpLaunch, PluginMcpSurface,
    PluginOkfSurface, PluginSkillSurface, PluginUiSurface, SurfaceActivation, ToolServiceSurface,
    ToolSurface, ToolTaskSource, ToolTaskSurface, ToolWorkload,
};
pub use registry::{
    validate_catalog_manifest_binding, ExtensionGenerationLease,
    ExtensionLifecycleGraphPublication, ExtensionLifecycleIdentity, ExtensionLifecyclePackage,
    ExtensionLifecycleResult, ExtensionLifecycleRollbackResult, ExtensionPackageBinding,
    ExtensionReceipt, ExtensionRegistry, ExtensionRegistryCutoverRecord, ExtensionRegistrySnapshot,
    ExtensionSnapshotCursor, ExtensionSnapshotLease, ExtensionSnapshotPackage, ExtensionTrust,
    InstalledExtension, UninstallResult, EXTENSION_RECEIPT_SCHEMA_VERSION,
    EXTENSION_REGISTRY_CUTOVER_SCHEMA, EXTENSION_SNAPSHOT_CURSOR_SCHEMA,
    MAX_PENDING_REGISTRY_CUTOVERS,
};
pub use registry_sources::{
    GitHubRegistryRepository, RegistrySource, RegistrySourceInput, RegistrySourceMutation,
    RegistrySourceSnapshot, RegistrySourceStore, ResolvedRegistrySources,
    DEFAULT_GITHUB_REGISTRY_PATH, DEFAULT_GITHUB_REGISTRY_REF, MAX_CONFIGURED_REGISTRY_SOURCES,
    REGISTRY_SOURCE_CONFIG_SCHEMA_VERSION,
};
pub use release_bundle::{
    inspect_release_bundle, ReleaseBundlePackage, RELEASE_BUNDLE_SCHEMA_VERSION,
};
pub use remote::{
    download_locked_cached_remote_packages, download_locked_remote_packages,
    download_selected_locked_cached_remote_packages, download_selected_locked_remote_packages,
    fetch_cached_cognitive_package_media, fetch_cognitive_package_media, inspect_bootstrap_root,
    inspect_cached_cognitive_package_presentation, inspect_cached_plugin,
    inspect_cognitive_package_presentation, inspect_remote_plugin, inspect_verified_target_cache,
    list_remote_packages, plugin_catalog_host_input_schema, plugin_catalog_inspection_input_schema,
    plugin_catalog_search_input_schema, prepare_cached_remote_package, prepare_remote_package,
    prune_verified_target_cache, refresh_remote_registry, resolve_cached_remote_package_lock,
    resolve_cached_remote_package_lock_with_observer, resolve_remote_package_lock,
    resolve_remote_package_lock_with_observer, search_cached_plugins, search_remote_plugins,
    CognitivePackageFormFactor, CognitivePackageMediaKind, CognitivePackagePresentationIndexV1,
    CognitivePackagePresentationMediaV1, CognitivePackagePresentationRecordV1,
    CognitivePackagePresentationV1, DownloadedRemotePackage, PackageRegistryResolutionObserver,
    PinnedBootstrapRoot, PluginCatalogAvailability, PluginCatalogHost, PluginCatalogInspection,
    PluginCatalogPage, PluginCatalogSearch, PluginCatalogSnapshot, PluginCatalogSnapshotSource,
    PreparedRemotePackage, RegistryNetworkPolicy, ResolvedRemotePackage, TrustedRegistry,
    VerifiedCognitivePackageMedia, VerifiedCognitivePackagePresentation, VerifiedRegistryCatalog,
    VerifiedRegistryMetadata, VerifiedTargetCachePolicy, VerifiedTargetCachePruneResult,
    VerifiedTargetCacheUsage, VerifiedTargetObservation, VerifiedTargetObservationStatus,
    COGNITIVE_PACKAGE_PRESENTATION_INDEX_SCHEMA, COGNITIVE_PACKAGE_PRESENTATION_SCHEMA,
    DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES, DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES,
    DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES, MAX_BOOTSTRAP_ROOT_BYTES,
    MAX_COGNITIVE_PACKAGE_MEDIA_BYTES, MAX_COGNITIVE_PACKAGE_PRESENTATION_MEDIA,
    MAX_PLUGIN_CATALOG_PAGE_BYTES, MAX_PLUGIN_CATALOG_PAGE_SIZE,
    VERIFIED_TARGET_CACHE_SCHEMA_VERSION,
};
pub use state_maintenance::{
    StateMaintenanceGuard, StateMaintenanceLock, ACTIVE_STATE_RESTORE_MARKER,
};
pub use surface_files::{
    inspect_flow_surface_file, inspect_mcp_surface_files, inspect_skill_surface_file,
    inspect_tool_surface_files, inspect_ui_surface_files, load_okf_bundle_files,
    PluginSurfaceFileEvidence,
};
pub use workspace_grant::{
    StoredWorkspaceGrant, WorkspaceGrantReceipt, WorkspaceGrantRevocation, WorkspaceGrantStore,
    WORKSPACE_GRANT_RECEIPT_SCHEMA, WORKSPACE_GRANT_REVOCATION_SCHEMA,
};
pub use workspace_grant_operation::{
    WorkspaceGrantCandidateCeiling, WorkspaceGrantCutoverEvidence, WorkspaceGrantLifecyclePhase,
    WorkspaceGrantOperationIntent, WorkspaceGrantOperationJournal, WorkspaceGrantPreparedCandidate,
    WorkspaceGrantRetirement, WorkspaceGrantRollbackEvidence, WORKSPACE_GRANT_CUTOVER_SCHEMA,
    WORKSPACE_GRANT_OPERATION_SCHEMA, WORKSPACE_GRANT_ROLLBACK_SCHEMA,
};

const RESERVED_ROUTES: &[&str] = &[
    "browser",
    "box",
    "capability",
    "ocr",
    "capabilities",
    "component",
    "registry",
    "extension",
    "doctor",
    "mcp",
    "help",
    "version",
];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionManifest {
    pub schema_version: u32,
    pub package_id: String,
    pub version: String,
    /// Optional human-facing CLI alias. It is never package, surface, or
    /// lifecycle ownership identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route_alias: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_use: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginPackageDependency>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<ExtensionRepository>,
    pub actions: Vec<RiskClass>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<PluginMcpSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub okf: Vec<PluginOkfSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<PluginFlowSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<PluginSkillSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<PluginUiSurface>,
}

/// One named cognitive-package contribution and its manifest-local
/// dependencies.
///
/// The package remains the lifecycle unit. Hosts use this inventory to stage
/// Tool, MCP, OKF, Flow, Skill, and UI contributions in dependency order and to
/// remove them in reverse order; a surface is never installed independently
/// from its owning package generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestPluginSurface {
    pub surface: PluginSurfaceRef,
    pub activation: SurfaceActivation,
    pub optional: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginSurfaceRef>,
}

/// Source repository identity carried by a versioned external capability
/// package. Trust still comes from the installation source and package digest;
/// this metadata lets hosts and users trace the capability back to its
/// canonical project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionRepository {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

impl ExtensionManifest {
    pub fn parse_acl(input: &str) -> UseResult<Self> {
        let document = a3s_acl::parse_acl(input).map_err(|error| {
            UseError::new(
                "use.extension.manifest_invalid",
                format!("Failed to parse extension ACL: {error}"),
            )
        })?;
        if document.blocks.len() != 1 || document.blocks[0].name != "extension" {
            return Err(UseError::new(
                "use.extension.manifest_invalid",
                "The manifest must contain only one extension block.",
            ));
        }
        let extension_blocks = document
            .blocks
            .iter()
            .filter(|block| block.name == "extension")
            .collect::<Vec<_>>();
        let [block] = extension_blocks.as_slice() else {
            return Err(UseError::new(
                "use.extension.manifest_invalid",
                "The manifest must contain exactly one extension block.",
            ));
        };
        parse_extension_block(block)
    }

    pub fn validate_package_root(&self, package_root: &Path) -> UseResult<()> {
        let mut paths = Vec::new();
        for tool in &self.tools {
            paths.extend(tool.package_paths());
        }
        for mcp in &self.mcp_servers {
            paths.extend(mcp.package_paths());
        }
        paths.extend(
            self.okf
                .iter()
                .map(|surface| Path::new(surface.bundle.root.as_str())),
        );
        for flow in &self.flows {
            paths.extend(flow.package_paths());
        }
        paths.extend(self.skills.iter().map(|surface| surface.path.as_path()));
        for ui in &self.ui {
            paths.extend(ui.package_paths());
        }

        for path in paths {
            validate_relative_path(path)?;
            let resolved = package_root.join(path);
            if !resolved.starts_with(package_root) {
                return Err(UseError::new(
                    "use.extension.path_escape",
                    format!("Surface path '{}' escapes the package.", path.display()),
                ));
            }
        }
        Ok(())
    }

    pub fn surface_kinds(&self) -> Vec<&'static str> {
        let mut surfaces = Vec::with_capacity(6);
        if !self.tools.is_empty() {
            surfaces.push("tool");
        }
        if !self.mcp_servers.is_empty() {
            surfaces.push("mcp");
        }
        if !self.okf.is_empty() {
            surfaces.push("okf");
        }
        if !self.flows.is_empty() {
            surfaces.push("flow");
        }
        if !self.skills.is_empty() {
            surfaces.push("skill");
        }
        if !self.ui.is_empty() {
            surfaces.push("ui");
        }
        surfaces
    }

    /// Return the complete cognitive-package surface graph in canonical identity
    /// order.
    ///
    /// Dependency validation happens while parsing the manifest. This method
    /// deliberately exposes one shared graph to reconciliation and lifecycle
    /// orchestration so their activation and removal rules cannot drift.
    pub fn plugin_surfaces(&self) -> UseResult<Vec<ManifestPluginSurface>> {
        let mut surfaces = Vec::with_capacity(
            self.tools.len()
                + self.mcp_servers.len()
                + self.okf.len()
                + self.flows.len()
                + self.skills.len()
                + self.ui.len(),
        );
        surfaces.extend(self.tools.iter().map(|surface| ManifestPluginSurface {
            surface: plugin_surface_ref(PluginSurfaceKind::Tool, &surface.id),
            activation: surface.activation,
            optional: surface.optional,
            dependencies: Vec::new(),
        }));
        surfaces.extend(
            self.mcp_servers
                .iter()
                .map(|surface| ManifestPluginSurface {
                    surface: plugin_surface_ref(PluginSurfaceKind::Mcp, &surface.id),
                    activation: surface.activation,
                    optional: surface.optional,
                    dependencies: Vec::new(),
                }),
        );
        surfaces.extend(self.okf.iter().map(|surface| ManifestPluginSurface {
            surface: plugin_surface_ref(PluginSurfaceKind::Okf, &surface.id),
            activation: SurfaceActivation::Eager,
            optional: surface.optional,
            dependencies: Vec::new(),
        }));
        surfaces.extend(self.flows.iter().map(|surface| {
            let mut dependencies = surface
                .requires_tools
                .iter()
                .map(|id| plugin_surface_ref(PluginSurfaceKind::Tool, id))
                .chain(
                    surface
                        .requires_mcp
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Mcp, id)),
                )
                .chain(
                    surface
                        .requires_okf
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Okf, id)),
                )
                .collect::<Vec<_>>();
            dependencies.sort();
            ManifestPluginSurface {
                surface: plugin_surface_ref(PluginSurfaceKind::Flow, &surface.id),
                activation: SurfaceActivation::Lazy,
                optional: surface.optional,
                dependencies,
            }
        }));
        surfaces.extend(self.skills.iter().map(|surface| {
            let mut dependencies = surface
                .requires_tools
                .iter()
                .map(|id| plugin_surface_ref(PluginSurfaceKind::Tool, id))
                .chain(
                    surface
                        .requires_mcp
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Mcp, id)),
                )
                .chain(
                    surface
                        .requires_okf
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Okf, id)),
                )
                .chain(
                    surface
                        .requires_flows
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Flow, id)),
                )
                .collect::<Vec<_>>();
            dependencies.sort();
            ManifestPluginSurface {
                surface: plugin_surface_ref(PluginSurfaceKind::Skill, &surface.id),
                activation: SurfaceActivation::Lazy,
                optional: surface.optional,
                dependencies,
            }
        }));
        surfaces.extend(self.ui.iter().map(|surface| {
            let mut dependencies = surface
                .skill
                .iter()
                .map(|id| plugin_surface_ref(PluginSurfaceKind::Skill, id))
                .chain(
                    surface
                        .bind_tools
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Tool, id)),
                )
                .chain(
                    surface
                        .bind_mcp
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Mcp, id)),
                )
                .chain(
                    surface
                        .bind_flows
                        .iter()
                        .map(|id| plugin_surface_ref(PluginSurfaceKind::Flow, id)),
                )
                .collect::<Vec<_>>();
            dependencies.sort();
            ManifestPluginSurface {
                surface: plugin_surface_ref(PluginSurfaceKind::Ui, &surface.id),
                activation: SurfaceActivation::Lazy,
                optional: surface.optional,
                dependencies,
            }
        }));
        surfaces.sort_by(|left, right| left.surface.cmp(&right.surface));

        let known = surfaces
            .iter()
            .map(|surface| surface.surface.clone())
            .collect::<BTreeSet<_>>();
        if known.len() != surfaces.len()
            || surfaces.iter().any(|surface| {
                surface
                    .dependencies
                    .iter()
                    .any(|dependency| !known.contains(dependency))
            })
        {
            return Err(manifest_error(
                "The named plugin surface graph is internally inconsistent.",
            ));
        }
        Ok(surfaces)
    }

    pub fn has_mcp(&self) -> bool {
        !self.mcp_servers.is_empty()
    }

    pub fn ui_count(&self) -> usize {
        self.ui.len()
    }

    pub fn supports_use_version(&self, version: &str) -> UseResult<bool> {
        let version = semver::Version::parse(version).map_err(|error| {
            manifest_error(format!("Invalid A3S Use host version '{version}': {error}"))
        })?;
        let Some(requirement) = &self.requires_use else {
            return Ok(true);
        };
        let requirement = semver::VersionReq::parse(requirement).map_err(|error| {
            manifest_error(format!(
                "Invalid A3S Use compatibility requirement '{requirement}': {error}"
            ))
        })?;
        Ok(requirement.matches(&version))
    }
}

fn plugin_surface_ref(kind: PluginSurfaceKind, id: &str) -> PluginSurfaceRef {
    PluginSurfaceRef {
        kind,
        id: id.to_string(),
    }
}

fn parse_extension_block(block: &Block) -> UseResult<ExtensionManifest> {
    require_known_attributes(
        block,
        &[
            "schema_version",
            "version",
            "route",
            "requires_use",
            "actions",
        ],
    )?;
    let package_id = block
        .labels
        .first()
        .cloned()
        .ok_or_else(|| manifest_error("The extension block requires a package ID label."))?;
    if block.labels.len() != 1 || !valid_package_id(&package_id) {
        return Err(manifest_error(
            "Package IDs must be '<publisher>/<name>' lowercase identifiers.",
        ));
    }
    let schema_number = number_attribute(block, "schema_version")?;
    if !schema_number.is_finite()
        || schema_number.fract() != 0.0
        || !(0.0..=u32::MAX as f64).contains(&schema_number)
    {
        return Err(manifest_error(
            "Extension schema_version must be a non-negative integer.",
        ));
    }
    let schema_version = schema_number as u32;
    if schema_version != 3 {
        return Err(manifest_error(
            "Only cognitive-package manifest schema version 3 is supported; rebuild packages created with a pre-release schema.",
        ));
    }
    let version = string_attribute(block, "version")?;
    semver::Version::parse(&version)
        .map_err(|error| manifest_error(format!("Invalid extension version: {error}")))?;
    let route_alias = optional_string_attribute(block, "route")?;
    if route_alias
        .as_deref()
        .is_some_and(|alias| !valid_route_alias(alias))
    {
        return Err(manifest_error(format!(
            "Extension route alias '{}' is invalid or reserved.",
            route_alias.as_deref().unwrap_or_default()
        )));
    }
    let action_names = list_attribute(block, "actions")?;
    if action_names.iter().collect::<BTreeSet<_>>().len() != action_names.len() {
        return Err(manifest_error("Action classes must be unique."));
    }
    let actions = action_names
        .into_iter()
        .map(|action| parse_risk_class(&action))
        .collect::<UseResult<Vec<_>>>()?;
    let mut seen = BTreeSet::new();
    let mut repository = None;
    let mut dependencies = Vec::new();
    let mut tools = Vec::new();
    let mut mcp_servers = Vec::new();
    let mut okf = Vec::new();
    let mut flows = Vec::new();
    let mut skills = Vec::new();
    let mut ui = Vec::new();
    for surface in &block.blocks {
        let name = surface.name.as_str();
        let singleton = name == "repository";
        if singleton && !seen.insert(name) {
            return Err(manifest_error(format!(
                "Duplicate '{}' surface.",
                surface.name
            )));
        }
        match name {
            "tool" => tools.push(plugin_manifest::parse_tool(surface)?),
            "mcp" => mcp_servers.push(plugin_manifest::parse_mcp(surface)?),
            "okf" => okf.push(plugin_manifest::parse_okf(surface)?),
            "flow" => flows.push(plugin_manifest::parse_flow(surface)?),
            "skill" => skills.push(plugin_manifest::parse_skill(surface)?),
            "ui" => ui.push(plugin_manifest::parse_ui(surface)?),
            "dependency" => dependencies.push(parse_package_dependency(surface)?),
            "repository" => repository = Some(parse_repository(surface)?),
            name => {
                return Err(manifest_error(format!(
                    "Unknown extension surface '{name}'."
                )))
            }
        }
    }
    if tools.is_empty()
        && mcp_servers.is_empty()
        && okf.is_empty()
        && flows.is_empty()
        && skills.is_empty()
        && ui.is_empty()
    {
        return Err(manifest_error(
            "A schema version 3 extension must declare Tool, MCP, OKF, Flow, Skill, and/or UI.",
        ));
    }
    plugin_manifest::validate_dependencies(&tools, &mcp_servers, &okf, &flows, &skills, &ui)?;
    dependencies.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    if dependencies
        .windows(2)
        .any(|pair| pair[0].package_id == pair[1].package_id)
    {
        return Err(manifest_error(
            "Package dependencies must be sorted and unique by package ID.",
        ));
    }
    if dependencies
        .iter()
        .any(|dependency| dependency.package_id == package_id)
    {
        return Err(manifest_error(
            "A cognitive package cannot depend on itself.",
        ));
    }
    PluginPackageDependency::validate_set(&package_id, &dependencies).map_err(|error| {
        manifest_error(format!("Invalid package dependency set: {}", error.message))
    })?;
    let requires_use = optional_string_attribute(block, "requires_use")?;
    let requirement = requires_use
        .as_deref()
        .ok_or_else(|| manifest_error("Cognitive-package manifests require 'requires_use'."))?;
    let requirement = semver::VersionReq::parse(requirement).map_err(|error| {
        manifest_error(format!(
            "Invalid A3S Use compatibility requirement '{requirement}': {error}"
        ))
    })?;
    if repository.is_none() {
        return Err(manifest_error(
            "Cognitive-package manifests require a repository block.",
        ));
    }
    let current_host = semver::Version::new(0, 3, 0);
    let obsolete_host = semver::Version::new(0, 2, 1);
    if !requirement.matches(&current_host) || requirement.matches(&obsolete_host) {
        return Err(manifest_error(
            "Schema version 3 must require A3S Use 0.3 and exclude pre-0.3 hosts.",
        ));
    }
    Ok(ExtensionManifest {
        schema_version,
        package_id,
        version,
        route_alias,
        requires_use,
        dependencies,
        repository,
        actions,
        tools,
        mcp_servers,
        okf,
        flows,
        skills,
        ui,
    })
}

fn parse_package_dependency(block: &Block) -> UseResult<PluginPackageDependency> {
    if block.labels.len() != 1 || !block.blocks.is_empty() {
        return Err(manifest_error(
            "A dependency block requires exactly one package ID label and no nested blocks.",
        ));
    }
    require_known_attributes(block, &["version"])?;
    PluginPackageDependency::new(block.labels[0].clone(), string_attribute(block, "version")?)
        .map_err(|error| {
            manifest_error(format!(
                "Package dependency '{}' must use a canonical semantic-version requirement: {}",
                block.labels[0], error.message
            ))
        })
}

fn parse_repository(block: &Block) -> UseResult<ExtensionRepository> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["url", "revision"])?;
    let url = string_attribute(block, "url")?;
    let parsed = url::Url::parse(&url)
        .map_err(|error| manifest_error(format!("Invalid repository URL '{url}': {error}")))?;
    if parsed.scheme() != "https"
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
        || parsed.host_str().is_none()
    {
        return Err(manifest_error(
            "Repository URLs must be credential-free HTTPS URLs without a query or fragment.",
        ));
    }
    let revision = optional_string_attribute(block, "revision")?;
    if revision.as_deref().is_some_and(|revision| {
        !matches!(revision.len(), 40 | 64)
            || !revision
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    }) {
        return Err(manifest_error(
            "Repository revisions must be lowercase 40- or 64-character commit digests.",
        ));
    }
    Ok(ExtensionRepository { url, revision })
}

fn bounded_text(value: String, label: &str, max_chars: usize) -> UseResult<String> {
    let value = value.trim().to_string();
    if value.is_empty() || value.chars().count() > max_chars {
        return Err(manifest_error(format!(
            "{label} must contain between 1 and {max_chars} characters."
        )));
    }
    Ok(value)
}

fn validate_relative_path(path: &Path) -> UseResult<()> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::CurDir
            )
        })
    {
        return Err(UseError::new(
            "use.extension.path_escape",
            format!("Surface path '{}' is not package-relative.", path.display()),
        ));
    }
    Ok(())
}

fn require_surface_shape(block: &Block) -> UseResult<()> {
    if !block.labels.is_empty() || !block.blocks.is_empty() {
        return Err(manifest_error(format!(
            "The '{}' surface cannot have labels or nested blocks.",
            block.name
        )));
    }
    Ok(())
}

fn require_known_attributes(block: &Block, allowed: &[&str]) -> UseResult<()> {
    if let Some(unknown) = block
        .attributes
        .keys()
        .find(|key| !allowed.contains(&key.as_str()))
    {
        return Err(manifest_error(format!(
            "Unknown '{}' attribute '{}'.",
            block.name, unknown
        )));
    }
    Ok(())
}

fn string_attribute(block: &Block, name: &str) -> UseResult<String> {
    block
        .attributes
        .get(name)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires string attribute '{name}'.",
                block.name
            ))
        })
}

fn number_attribute(block: &Block, name: &str) -> UseResult<f64> {
    block
        .attributes
        .get(name)
        .and_then(Value::as_number)
        .ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires numeric attribute '{name}'.",
                block.name
            ))
        })
}

fn optional_bool_attribute(block: &Block, name: &str) -> UseResult<Option<bool>> {
    match block.attributes.get(name) {
        None => Ok(None),
        Some(value) => value.as_bool().map(Some).ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires boolean attribute '{name}'.",
                block.name
            ))
        }),
    }
}

fn optional_string_attribute(block: &Block, name: &str) -> UseResult<Option<String>> {
    match block.attributes.get(name) {
        None => Ok(None),
        Some(value) => value.as_str().map(str::to_string).map(Some).ok_or_else(|| {
            manifest_error(format!(
                "'{}' requires string attribute '{name}'.",
                block.name
            ))
        }),
    }
}

fn optional_i32_attribute(block: &Block, name: &str) -> UseResult<Option<i32>> {
    let Some(value) = block.attributes.get(name) else {
        return Ok(None);
    };
    let Some(value) = value.as_number() else {
        return Err(manifest_error(format!(
            "'{}' requires numeric attribute '{name}'.",
            block.name
        )));
    };
    if !value.is_finite()
        || value.fract() != 0.0
        || !(i32::MIN as f64..=i32::MAX as f64).contains(&value)
    {
        return Err(manifest_error(format!(
            "'{}' attribute '{name}' must be a 32-bit integer.",
            block.name
        )));
    }
    Ok(Some(value as i32))
}

fn list_attribute(block: &Block, name: &str) -> UseResult<Vec<String>> {
    let Some(Value::List(values)) = block.attributes.get(name) else {
        return Err(manifest_error(format!(
            "'{}' requires list attribute '{name}'.",
            block.name
        )));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| manifest_error(format!("'{name}' accepts only strings.")))
        })
        .collect()
}

fn optional_list_attribute(block: &Block, name: &str) -> UseResult<Vec<String>> {
    if block.attributes.contains_key(name) {
        list_attribute(block, name)
    } else {
        Ok(Vec::new())
    }
}

fn parse_risk_class(value: &str) -> UseResult<RiskClass> {
    match value {
        "read" => Ok(RiskClass::Read),
        "navigate" => Ok(RiskClass::Navigate),
        "mutate" => Ok(RiskClass::Mutate),
        "submit" => Ok(RiskClass::Submit),
        "download" => Ok(RiskClass::Download),
        "execute" => Ok(RiskClass::Execute),
        _ => Err(manifest_error(format!("Unknown action class '{value}'."))),
    }
}

fn valid_package_id(value: &str) -> bool {
    let segments = value.split('/').collect::<Vec<_>>();
    segments.len() == 2 && segments.into_iter().all(valid_segment)
}

pub(crate) fn valid_route_alias(value: &str) -> bool {
    valid_segment(value) && !RESERVED_ROUTES.contains(&value)
}

fn valid_segment(value: &str) -> bool {
    let mut characters = value.chars();
    matches!(characters.next(), Some(first) if first.is_ascii_lowercase())
        && characters.all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

fn manifest_error(message: impl Into<String>) -> UseError {
    UseError::new("use.extension.manifest_invalid", message)
}
