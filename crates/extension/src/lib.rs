use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

use a3s_acl::{Block, Value};
use a3s_use_core::{RiskClass, UseError, UseResult};
use serde::{Deserialize, Serialize};

mod digest;
#[cfg(test)]
mod manifest_tests;
mod package;
#[cfg(test)]
mod package_tests;
mod paths;
mod plugin_manifest;
#[cfg(test)]
mod plugin_manifest_tests;
mod registry;
mod registry_io;
mod release_bundle;
mod remote;
mod route_lock;
mod source;
mod surface_files;
mod workspace_grant;
mod workspace_grant_io;
mod workspace_grant_snapshot;
#[cfg(test)]
mod workspace_grant_tests;

pub use paths::ExtensionPaths;
pub use plugin_manifest::{
    PluginMcpLaunch, PluginMcpSurface, PluginSkillSurface, PluginUiSurface, SurfaceActivation,
    ToolServiceSurface, ToolSurface, ToolTaskSource, ToolTaskSurface, ToolWorkload,
};
pub use registry::{
    ActivationResult, ExtensionReceipt, ExtensionRegistry, ExtensionRegistrySnapshot,
    ExtensionRouteBinding, ExtensionRouteLease, ExtensionTrust, InstallOptions, InstallResult,
    InstalledExtension, UninstallResult,
};
pub use release_bundle::{
    inspect_release_bundle, ReleaseBundlePackage, RELEASE_BUNDLE_SCHEMA_VERSION,
};
pub use remote::{
    inspect_cached_plugin, inspect_remote_plugin, list_remote_packages, prepare_remote_package,
    refresh_remote_registry, search_cached_plugins, search_remote_plugins, DownloadedRemotePackage,
    PluginCatalogAvailability, PluginCatalogHost, PluginCatalogInspection, PluginCatalogPage,
    PluginCatalogSearch, PluginCatalogSnapshot, PluginCatalogSnapshotSource, PreparedRemotePackage,
    ResolvedRemotePackage, TrustedRegistry, VerifiedRegistryCatalog, VerifiedRegistryMetadata,
    MAX_PLUGIN_CATALOG_PAGE_BYTES, MAX_PLUGIN_CATALOG_PAGE_SIZE,
};
pub use workspace_grant::{
    StoredWorkspaceGrant, WorkspaceGrantReceipt, WorkspaceGrantRevocation, WorkspaceGrantStore,
    WORKSPACE_GRANT_RECEIPT_SCHEMA, WORKSPACE_GRANT_REVOCATION_SCHEMA,
};

const RESERVED_ROUTES: &[&str] = &[
    "browser",
    "box",
    "capability",
    "ocr",
    "capabilities",
    "component",
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
    pub route: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<ExtensionRepository>,
    pub actions: Vec<RiskClass>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cli: Option<CliSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpSurface>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<SkillSurface>,
    #[serde(default, skip_serializing_if = "ExtensionContributions::is_empty")]
    pub contributes: ExtensionContributions,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<ToolSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<PluginMcpSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<PluginSkillSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ui: Vec<PluginUiSurface>,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CliSurface {
    pub executable: PathBuf,
    pub json_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSurface {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSurface {
    pub path: PathBuf,
}

/// Declarative, non-callable Workbench contributions owned by this package.
///
/// Runtime actions remain limited to native CLI, standard MCP, and Skill
/// surfaces. Contributions only describe integrity-bound assets that a host
/// may choose to render in its own security boundary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtensionContributions {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity_bar: Vec<ActivityBarContribution>,
}

impl ExtensionContributions {
    pub fn is_empty(&self) -> bool {
        self.activity_bar.is_empty()
    }
}

/// One VS Code-style Activity Bar contribution backed by a managed HTML
/// asset and the extension's installed Skill guidance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityBarContribution {
    pub id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    pub icon: String,
    pub entry: PathBuf,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<PathBuf>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<PathBuf>,
    pub skill: String,
    pub order: i32,
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
        paths.extend(self.cli.iter().map(|surface| surface.executable.as_path()));
        paths.extend(self.mcp.iter().map(|surface| surface.executable.as_path()));
        paths.extend(self.skill.iter().map(|surface| surface.path.as_path()));
        for contribution in &self.contributes.activity_bar {
            paths.push(contribution.entry.as_path());
            paths.extend(contribution.styles.iter().map(PathBuf::as_path));
            paths.extend(contribution.scripts.iter().map(PathBuf::as_path));
        }
        for tool in &self.tools {
            paths.extend(tool.package_paths());
        }
        for mcp in &self.mcp_servers {
            paths.extend(mcp.package_paths());
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
        let mut surfaces = Vec::with_capacity(4);
        if self.cli.is_some() {
            surfaces.push("cli");
        }
        if !self.tools.is_empty() {
            surfaces.push("tool");
        }
        if self.mcp.is_some() || !self.mcp_servers.is_empty() {
            surfaces.push("mcp");
        }
        if self.skill.is_some() || !self.skills.is_empty() {
            surfaces.push("skill");
        }
        if !self.ui.is_empty() {
            surfaces.push("ui");
        }
        surfaces
    }

    pub fn has_mcp(&self) -> bool {
        self.mcp.is_some() || !self.mcp_servers.is_empty()
    }

    pub fn ui_count(&self) -> usize {
        self.contributes.activity_bar.len() + self.ui.len()
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
    if !(1..=3).contains(&schema_version) {
        return Err(manifest_error(
            "Only extension schema versions 1, 2, and 3 are supported.",
        ));
    }
    let version = string_attribute(block, "version")?;
    semver::Version::parse(&version)
        .map_err(|error| manifest_error(format!("Invalid extension version: {error}")))?;
    let route = string_attribute(block, "route")?;
    if !valid_segment(&route) || RESERVED_ROUTES.contains(&route.as_str()) {
        return Err(manifest_error(format!(
            "Extension route '{route}' is invalid or reserved."
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
    let mut cli = None;
    let mut mcp = None;
    let mut skill = None;
    let mut repository = None;
    let mut contributes = ExtensionContributions::default();
    let mut tools = Vec::new();
    let mut mcp_servers = Vec::new();
    let mut skills = Vec::new();
    let mut ui = Vec::new();
    for surface in &block.blocks {
        let name = surface.name.as_str();
        let singleton = matches!(name, "cli" | "repository" | "contributes")
            || (schema_version < 3 && matches!(name, "mcp" | "skill"));
        if singleton && !seen.insert(name) {
            return Err(manifest_error(format!(
                "Duplicate '{}' surface.",
                surface.name
            )));
        }
        match name {
            "cli" if schema_version == 3 => {
                return Err(manifest_error(
                    "Schema version 3 cannot declare legacy 'cli' or 'contributes' surfaces.",
                ))
            }
            "cli" => cli = Some(parse_cli(surface)?),
            "tool" if schema_version == 3 => {
                tools.push(plugin_manifest::parse_tool(surface)?);
            }
            "mcp" if schema_version == 3 => {
                mcp_servers.push(plugin_manifest::parse_mcp(surface)?);
            }
            "mcp" => mcp = Some(parse_mcp(surface)?),
            "skill" if schema_version == 3 => {
                skills.push(plugin_manifest::parse_skill(surface)?);
            }
            "skill" => skill = Some(parse_skill(surface)?),
            "ui" if schema_version == 3 => {
                ui.push(plugin_manifest::parse_ui(surface)?);
            }
            "repository" => repository = Some(parse_repository(surface)?),
            "contributes" if schema_version == 3 => {
                return Err(manifest_error(
                    "Schema version 3 cannot declare legacy 'cli' or 'contributes' surfaces.",
                ))
            }
            "contributes" => contributes = parse_contributes(surface)?,
            name => {
                return Err(manifest_error(format!(
                    "Unknown extension surface '{name}'."
                )))
            }
        }
    }
    if schema_version < 3 && cli.is_none() && mcp.is_none() && skill.is_none() {
        return Err(manifest_error(
            "An extension must declare CLI, MCP, and/or Skill.",
        ));
    }
    if schema_version == 3
        && tools.is_empty()
        && mcp_servers.is_empty()
        && skills.is_empty()
        && ui.is_empty()
    {
        return Err(manifest_error(
            "A schema version 3 extension must declare Tool, MCP, Skill, and/or UI.",
        ));
    }
    if schema_version == 3 {
        plugin_manifest::validate_dependencies(&tools, &mcp_servers, &skills, &ui)?;
    }
    if !contributes.activity_bar.is_empty() && skill.is_none() {
        return Err(manifest_error(
            "Activity Bar contributions require a Skill surface from the same extension.",
        ));
    }
    let requires_use = optional_string_attribute(block, "requires_use")?;
    if schema_version == 1 && (requires_use.is_some() || repository.is_some()) {
        return Err(manifest_error(
            "Repository identity and A3S Use compatibility require extension schema version 2.",
        ));
    }
    if schema_version >= 2 {
        let requirement = requires_use.as_deref().ok_or_else(|| {
            manifest_error(format!(
                "Extension schema version {schema_version} requires 'requires_use'."
            ))
        })?;
        let requirement = semver::VersionReq::parse(requirement).map_err(|error| {
            manifest_error(format!(
                "Invalid A3S Use compatibility requirement '{requirement}': {error}"
            ))
        })?;
        if repository.is_none() {
            return Err(manifest_error(format!(
                "Extension schema version {schema_version} requires a repository block."
            )));
        }
        if schema_version == 3 {
            let v3_host = semver::Version::new(0, 3, 0);
            let current_host =
                semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|error| {
                    manifest_error(format!(
                        "The A3S Use package version is invalid during schema validation: {error}"
                    ))
                })?;
            if !requirement.matches(&v3_host)
                || (current_host < v3_host && requirement.matches(&current_host))
            {
                return Err(manifest_error(
                    "Schema version 3 must require A3S Use 0.3 and exclude pre-0.3 hosts.",
                ));
            }
        }
    }
    Ok(ExtensionManifest {
        schema_version,
        package_id,
        version,
        route,
        requires_use,
        repository,
        actions,
        cli,
        mcp,
        skill,
        contributes,
        tools,
        mcp_servers,
        skills,
        ui,
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

fn parse_cli(block: &Block) -> UseResult<CliSurface> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["executable", "json_output"])?;
    let executable = PathBuf::from(string_attribute(block, "executable")?);
    validate_relative_path(&executable)?;
    Ok(CliSurface {
        executable,
        json_output: optional_bool_attribute(block, "json_output")?.unwrap_or(false),
    })
}

fn parse_mcp(block: &Block) -> UseResult<McpSurface> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["executable", "args", "transport"])?;
    let executable = PathBuf::from(string_attribute(block, "executable")?);
    validate_relative_path(&executable)?;
    let transport = match string_attribute(block, "transport")?.as_str() {
        "stdio" => McpTransport::Stdio,
        "streamable-http" => McpTransport::StreamableHttp,
        value => {
            return Err(manifest_error(format!(
                "Unsupported MCP transport '{value}'."
            )))
        }
    };
    Ok(McpSurface {
        executable,
        args: optional_list_attribute(block, "args")?,
        transport,
    })
}

fn parse_skill(block: &Block) -> UseResult<SkillSurface> {
    require_surface_shape(block)?;
    require_known_attributes(block, &["path"])?;
    let path = PathBuf::from(string_attribute(block, "path")?);
    validate_relative_path(&path)?;
    if path.file_name().and_then(|value| value.to_str()) != Some("SKILL.md") {
        return Err(manifest_error("Skill surfaces must point to SKILL.md."));
    }
    Ok(SkillSurface { path })
}

fn parse_contributes(block: &Block) -> UseResult<ExtensionContributions> {
    if !block.labels.is_empty() {
        return Err(manifest_error(
            "The 'contributes' block cannot have labels.",
        ));
    }
    require_known_attributes(block, &[])?;
    let mut activity_bar = Vec::new();
    let mut ids = BTreeSet::new();
    for contribution in &block.blocks {
        if contribution.name != "activity_bar" {
            return Err(manifest_error(format!(
                "Unknown contribution point '{}'.",
                contribution.name
            )));
        }
        let activity = parse_activity_bar(contribution)?;
        if !ids.insert(activity.id.clone()) {
            return Err(manifest_error(format!(
                "Duplicate Activity Bar contribution '{}'.",
                activity.id
            )));
        }
        activity_bar.push(activity);
    }
    Ok(ExtensionContributions { activity_bar })
}

fn parse_activity_bar(block: &Block) -> UseResult<ActivityBarContribution> {
    if !block.blocks.is_empty() || block.labels.len() != 1 {
        return Err(manifest_error(
            "An 'activity_bar' contribution requires one ID label and no nested blocks.",
        ));
    }
    require_known_attributes(
        block,
        &[
            "title",
            "description",
            "icon",
            "entry",
            "styles",
            "scripts",
            "skill",
            "order",
        ],
    )?;
    let id = block.labels[0].clone();
    if !valid_segment(&id) {
        return Err(manifest_error(format!(
            "Activity Bar contribution ID '{id}' is invalid."
        )));
    }
    let title = bounded_text(string_attribute(block, "title")?, "Activity Bar title", 64)?;
    let description = optional_string_attribute(block, "description")?
        .map(|value| bounded_text(value, "Activity Bar description", 240))
        .transpose()?
        .unwrap_or_default();
    let icon = string_attribute(block, "icon")?;
    if !valid_segment(&icon) {
        return Err(manifest_error(format!(
            "Activity Bar icon '{icon}' must be a lowercase icon identifier."
        )));
    }
    let entry = PathBuf::from(string_attribute(block, "entry")?);
    validate_relative_path(&entry)?;
    if entry.extension().and_then(|value| value.to_str()) != Some("html") {
        return Err(manifest_error(
            "Activity Bar entry assets must be .html files.",
        ));
    }
    let styles = activity_resource_paths(block, "styles", "css")?;
    let scripts = activity_resource_paths(block, "scripts", "js")?;
    let mut resources = BTreeSet::from([entry.clone()]);
    for resource in styles.iter().chain(&scripts) {
        if !resources.insert(resource.clone()) {
            return Err(manifest_error(format!(
                "Activity Bar asset '{}' is declared more than once.",
                resource.display()
            )));
        }
    }
    let skill = string_attribute(block, "skill")?;
    if !valid_segment(&skill) {
        return Err(manifest_error(format!(
            "Activity Bar Skill name '{skill}' is invalid."
        )));
    }
    let order = optional_i32_attribute(block, "order")?.unwrap_or(100);
    Ok(ActivityBarContribution {
        id,
        title,
        description,
        icon,
        entry,
        styles,
        scripts,
        skill,
        order,
    })
}

fn activity_resource_paths(block: &Block, name: &str, extension: &str) -> UseResult<Vec<PathBuf>> {
    let values = optional_list_attribute(block, name)?;
    if values.len() > 16 {
        return Err(manifest_error(format!(
            "Activity Bar '{name}' accepts at most 16 assets."
        )));
    }
    let mut seen = BTreeSet::new();
    values
        .into_iter()
        .map(|value| {
            let path = PathBuf::from(value);
            validate_relative_path(&path)?;
            if path.extension().and_then(|value| value.to_str()) != Some(extension) {
                return Err(manifest_error(format!(
                    "Activity Bar '{name}' assets must use the .{extension} extension."
                )));
            }
            if !seen.insert(path.clone()) {
                return Err(manifest_error(format!(
                    "Activity Bar asset '{}' is declared more than once.",
                    path.display()
                )));
            }
            Ok(path)
        })
        .collect()
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
