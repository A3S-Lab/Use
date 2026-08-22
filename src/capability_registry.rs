//! Unified projection of built-in and externally installed Use capabilities.
//!
//! This is a versioned JSON CLI contract for long-running consumers. It is
//! not a private RPC protocol: invocation still happens through native CLI,
//! standard MCP, and `SKILL.md` surfaces.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use a3s_use_core::{
    InstalledPluginPlanEvidence, OkfCapabilityProjection, PlanScope, PluginSurfaceRef, Readiness,
    UseError, UseResult,
};
#[cfg(feature = "extensions")]
use a3s_use_core::{
    PlanQualifiedSurfaceRef, PluginSurfaceKind, INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA,
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[cfg(feature = "extensions")]
#[path = "capability_registry/runtime_tasks.rs"]
mod runtime_tasks;
#[cfg(feature = "extensions")]
use runtime_tasks::runtime_task_evidence_from_store;

#[cfg(feature = "extensions")]
use crate::surface_reconciler::{
    reconcile_with_runtime_and_knowledge, PluginDesiredState, PluginObservedState,
    SurfaceObservations, SurfaceObservedState, SurfaceReconcileSnapshot,
};
#[cfg(feature = "extensions")]
use crate::{
    cognitive_package::COGNITIVE_PACKAGE_DEFAULT_SCOPE,
    flow_runtime::FlowRuntimeBindingStore,
    okf_knowledge::{
        OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient,
        SqliteOkfKnowledgeAdapter,
    },
    plugin_runtime::RuntimeBindingStore,
};

const SCHEMA_VERSION: u32 = 2;
#[cfg(feature = "extensions")]
const PLANNER_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const WATCH_INTERVAL: Duration = Duration::from_millis(100);
const MAX_STABLE_SNAPSHOT_ATTEMPTS: usize = 5;
const MAX_FLOW_SOURCE_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityRegistrySnapshot {
    pub schema_version: u32,
    pub generation: u64,
    pub revision: String,
    pub capabilities: Vec<CapabilityBinding>,
}

impl CapabilityRegistrySnapshot {
    #[cfg(feature = "extensions")]
    pub(crate) fn knowledge_projections(&self) -> Vec<OkfCapabilityProjection> {
        let mut projections = self
            .capabilities
            .iter()
            .flat_map(|capability| capability.knowledge.iter().cloned())
            .collect::<Vec<_>>();
        projections.sort_by(|left, right| {
            left.surface
                .cmp(&right.surface)
                .then_with(|| left.generation.cmp(&right.generation))
        });
        projections
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CapabilityOrigin {
    BuiltIn,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum McpTransport {
    Stdio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct McpSurface {
    target: String,
    transport: McpTransport,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SkillSurface {
    path: PathBuf,
    sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum FlowEngine {
    A3sFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum FlowRuntime {
    NativeTs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct FlowSurface {
    id: String,
    engine: FlowEngine,
    runtime: FlowRuntime,
    source: ManagedAsset,
    export_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires_mcp: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    requires_okf: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectedLifecycleIdentity {
    package_id: String,
    package_digest: String,
    manifest_digest: String,
    generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ToolTaskProjection {
    tool_name: String,
    surface_id: String,
    command: String,
    json_output: bool,
    timeout_ms: u64,
    scope: PlanScope,
    lifecycle_identity: ProjectedLifecycleIdentity,
    provider_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ManagedAsset {
    path: PathBuf,
    sha256: String,
    media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ActivityBarContribution {
    id: String,
    title: String,
    description: String,
    icon: String,
    entry: ManagedAsset,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    styles: Vec<ManagedAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    scripts: Vec<ManagedAsset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill: Option<String>,
    order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
struct RepositoryBinding {
    url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PluginPlannerEvidence {
    pub schema_version: u32,
    pub package_id: String,
    pub package_sha256: String,
    pub manifest_sha256: String,
    pub receipt_digest: String,
    pub catalog_record_digest: String,
    pub desired_enabled: bool,
    pub selected_surfaces: Vec<PluginSurfaceRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CapabilityBinding {
    id: String,
    route: String,
    version: String,
    origin: CapabilityOrigin,
    enabled: bool,
    readiness: Readiness,
    #[cfg(feature = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    reconciliation: Option<SurfaceReconcileSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    planner_evidence: Option<PluginPlannerEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    package_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    lifecycle_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    requires_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    repository: Option<RepositoryBinding>,
    surfaces: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mcp: Option<McpSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    skills: Vec<SkillSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    flows: Vec<FlowSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    knowledge: Vec<OkfCapabilityProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    activity_bar: Vec<ActivityBarContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    tool_tasks: Vec<ToolTaskProjection>,
}

pub(crate) async fn snapshot() -> UseResult<CapabilityRegistrySnapshot> {
    let (generation, extensions) = stable_extensions().await?;
    let mut capabilities = vec![
        browser_capability().await?,
        ocr_capability().await?,
        box_capability(),
    ];
    capabilities.extend(extensions);
    capabilities.sort_by(|left, right| left.id.cmp(&right.id));
    validate_unique_tool_task_names(&capabilities)?;

    let revision = revision(&capabilities)?;
    Ok(CapabilityRegistrySnapshot {
        schema_version: SCHEMA_VERSION,
        generation,
        revision,
        capabilities,
    })
}

fn validate_unique_tool_task_names(capabilities: &[CapabilityBinding]) -> UseResult<()> {
    let mut names = std::collections::BTreeSet::new();
    for task in capabilities
        .iter()
        .flat_map(|capability| capability.tool_tasks.iter())
    {
        if !names.insert(task.tool_name.as_str()) {
            return Err(UseError::new(
                "use.capability.runtime_task_name_conflict",
                "Two Runtime Tool Tasks resolve to the same host tool identity.",
            ));
        }
    }
    Ok(())
}

#[cfg(feature = "extensions")]
pub(crate) async fn installed_plugin_plan_evidence(
    package_id: &str,
) -> UseResult<InstalledPluginPlanEvidence> {
    let snapshot = snapshot().await?;
    let extension = crate::extension_host::get(package_id)
        .await?
        .ok_or_else(|| {
            UseError::new(
                "use.extension.not_installed",
                format!("Extension '{package_id}' is not installed."),
            )
        })?;
    installed_plugin_plan_evidence_from_snapshot(&snapshot, &extension)
}

#[cfg(not(feature = "extensions"))]
pub(crate) async fn installed_plugin_plan_evidence(
    _package_id: &str,
) -> UseResult<InstalledPluginPlanEvidence> {
    Err(UseError::new(
        "use.extension.disabled",
        "External extension support is disabled in this custom build.",
    ))
}

pub(crate) async fn wait_for_change(
    after_generation: u64,
    after_revision: Option<&str>,
    timeout: Duration,
) -> UseResult<Option<CapabilityRegistrySnapshot>> {
    let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
        UseError::new(
            "use.capability.timeout_invalid",
            "The capability watch timeout is too large.",
        )
    })?;

    loop {
        let current = snapshot().await?;
        let changed = match after_revision {
            Some(revision) => {
                current.generation != after_generation || current.revision != revision
            }
            None => current.generation > after_generation,
        };
        if changed {
            return Ok(Some(current));
        }

        let now = Instant::now();
        if now >= deadline {
            return Ok(None);
        }
        tokio::time::sleep(WATCH_INTERVAL.min(deadline.saturating_duration_since(now))).await;
    }
}

async fn browser_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "browser")]
    {
        let diagnostic = a3s_use_browser::doctor();
        let skill = crate::browser_driver::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface(path).await?]),
            None => (None, Vec::new()),
        };
        Ok(CapabilityBinding {
            id: "use/browser".to_string(),
            route: "browser".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces: vec!["cli".to_string(), "mcp".to_string(), "skill".to_string()],
            mcp: crate::browser_driver::is_available().then(|| McpSurface {
                target: "browser".to_string(),
                transport: McpTransport::Stdio,
            }),
            skills,
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
        })
    }
    #[cfg(not(feature = "browser"))]
    {
        Ok(CapabilityBinding {
            id: "use/browser".to_string(),
            route: "browser".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root: None,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces: Vec::new(),
            mcp: None,
            skills: Vec::new(),
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
        })
    }
}

async fn ocr_capability() -> UseResult<CapabilityBinding> {
    #[cfg(feature = "ocr")]
    {
        let diagnostic = crate::ocr_builtin::diagnostic();
        let skill = crate::ocr_builtin::primary_skill_surface().await;
        let (package_root, skills) = match skill {
            Some((root, path)) => (Some(root), vec![skill_surface(path).await?]),
            None => (None, Vec::new()),
        };
        let mut surfaces = vec!["cli".to_string()];
        if !skills.is_empty() {
            surfaces.push("skill".to_string());
        }
        #[cfg(feature = "mcp")]
        surfaces.push("mcp".to_string());
        Ok(CapabilityBinding {
            id: "use/ocr".to_string(),
            route: "ocr".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: true,
            readiness: diagnostic.readiness,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces,
            #[cfg(feature = "mcp")]
            mcp: Some(McpSurface {
                target: "ocr-native".to_string(),
                transport: McpTransport::Stdio,
            }),
            #[cfg(not(feature = "mcp"))]
            mcp: None,
            skills,
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
        })
    }
    #[cfg(not(feature = "ocr"))]
    {
        Ok(CapabilityBinding {
            id: "use/ocr".to_string(),
            route: "ocr".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            origin: CapabilityOrigin::BuiltIn,
            enabled: false,
            readiness: Readiness::Missing,
            #[cfg(feature = "extensions")]
            reconciliation: None,
            planner_evidence: None,
            package_root: None,
            lifecycle_generation: None,
            requires_use: None,
            repository: None,
            surfaces: Vec::new(),
            mcp: None,
            skills: Vec::new(),
            flows: Vec::new(),
            knowledge: Vec::new(),
            activity_bar: Vec::new(),
            tool_tasks: Vec::new(),
        })
    }
}

fn box_capability() -> CapabilityBinding {
    let diagnostic = crate::component_route::box_diagnostic();
    CapabilityBinding {
        id: "use/box".to_string(),
        route: "box".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        origin: CapabilityOrigin::BuiltIn,
        enabled: diagnostic.readiness == Readiness::Ready,
        readiness: diagnostic.readiness,
        #[cfg(feature = "extensions")]
        reconciliation: None,
        planner_evidence: None,
        package_root: None,
        lifecycle_generation: None,
        requires_use: None,
        repository: None,
        surfaces: vec!["cli".to_string()],
        mcp: None,
        skills: Vec::new(),
        flows: Vec::new(),
        knowledge: Vec::new(),
        activity_bar: Vec::new(),
        tool_tasks: Vec::new(),
    }
}

fn revision(capabilities: &[CapabilityBinding]) -> UseResult<String> {
    let bytes = serde_json::to_vec(capabilities).map_err(|error| {
        UseError::new(
            "use.capability.snapshot_invalid",
            format!("Failed to encode the capability snapshot: {error}"),
        )
    })?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

async fn skill_surface(path: PathBuf) -> UseResult<SkillSurface> {
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| skill_io_error("inspect", &path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(UseError::new(
            "use.capability.skill_invalid",
            format!(
                "Projected Skill '{}' must be a regular package file.",
                path.display()
            ),
        ));
    }

    let mut file = tokio::fs::File::open(&path)
        .await
        .map_err(|error| skill_io_error("open", &path, error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .await
            .map_err(|error| skill_io_error("read", &path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }

    Ok(SkillSurface {
        path,
        sha256: format!("{:x}", digest.finalize()),
    })
}

async fn activity_asset(path: PathBuf, media_type: &str) -> UseResult<ManagedAsset> {
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| activity_io_error("inspect", &path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(UseError::new(
            "use.capability.activity_asset_invalid",
            format!(
                "Projected Activity Bar asset '{}' must be a regular package file.",
                path.display()
            ),
        ));
    }
    if metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 {
        return Err(UseError::new(
            "use.capability.activity_asset_invalid",
            format!(
                "Projected Activity Bar asset '{}' exceeds the supported size.",
                path.display()
            ),
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| activity_io_error("read", &path, error))?;
    std::str::from_utf8(&bytes).map_err(|error| {
        UseError::new(
            "use.capability.activity_asset_invalid",
            format!(
                "Projected Activity Bar asset '{}' must be UTF-8 {media_type}: {error}",
                path.display(),
            ),
        )
    })?;
    Ok(ManagedAsset {
        path,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        media_type: media_type.to_string(),
    })
}

async fn flow_asset(path: PathBuf) -> UseResult<ManagedAsset> {
    let metadata = tokio::fs::symlink_metadata(&path)
        .await
        .map_err(|error| flow_io_error("inspect", &path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(UseError::new(
            "use.capability.flow_source_invalid",
            format!(
                "Projected A3S Flow source '{}' must be a regular package file.",
                path.display()
            ),
        ));
    }
    if metadata.len() == 0 || metadata.len() > MAX_FLOW_SOURCE_BYTES {
        return Err(UseError::new(
            "use.capability.flow_source_invalid",
            format!(
                "Projected A3S Flow source '{}' exceeds the supported size.",
                path.display()
            ),
        ));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| flow_io_error("read", &path, error))?;
    std::str::from_utf8(&bytes).map_err(|error| {
        UseError::new(
            "use.capability.flow_source_invalid",
            format!(
                "Projected A3S Flow source '{}' must be UTF-8 TypeScript: {error}",
                path.display(),
            ),
        )
    })?;
    Ok(ManagedAsset {
        path,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        media_type: "text/typescript".to_string(),
    })
}

fn activity_io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.capability.activity_asset_unreadable",
        format!(
            "Failed to {action} projected Activity Bar asset '{}': {error}",
            path.display()
        ),
    )
}

fn flow_io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.capability.flow_source_unreadable",
        format!(
            "Failed to {action} projected A3S Flow source '{}': {error}",
            path.display()
        ),
    )
}

fn skill_io_error(action: &str, path: &Path, error: std::io::Error) -> UseError {
    UseError::new(
        "use.capability.skill_unreadable",
        format!(
            "Failed to {action} projected Skill '{}': {error}",
            path.display()
        ),
    )
}

#[cfg(feature = "extensions")]
async fn stable_extensions() -> UseResult<(u64, Vec<CapabilityBinding>)> {
    for _ in 0..MAX_STABLE_SNAPSHOT_ATTEMPTS {
        let before = crate::extension_host::snapshot().await?;
        let Some(capabilities) = project_extensions(&before).await? else {
            continue;
        };
        let after = crate::extension_host::snapshot().await?;
        if before == after {
            return Ok((before.generation, capabilities));
        }
    }
    Err(UseError::new(
        "use.capability.registry_busy",
        "The extension registry changed repeatedly while capabilities were projected.",
    )
    .with_suggestion("Retry the capability snapshot after the current component operation."))
}

#[cfg(not(feature = "extensions"))]
async fn stable_extensions() -> UseResult<(u64, Vec<CapabilityBinding>)> {
    Ok((0, Vec::new()))
}

#[cfg(feature = "extensions")]
async fn project_extensions(
    snapshot: &a3s_use_extension::ExtensionRegistrySnapshot,
) -> UseResult<Option<Vec<CapabilityBinding>>> {
    let mut capabilities = Vec::with_capacity(snapshot.routes.len());
    for route in &snapshot.routes {
        let Some(extension) = crate::extension_host::get_snapshot_binding(route).await? else {
            return Ok(None);
        };
        let receipt = &extension.receipt;
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if receipt.package_id != route.package_id
            || receipt.component_id != route.component_id
            || receipt.route != route.route
            || receipt.version != route.version
            || receipt.package_root != route.package_root
            || receipt.manifest_sha256 != route.manifest_sha256
            || receipt.lifecycle_generation != route.lifecycle_generation
            || receipt.enabled != route.enabled
            || surfaces != route.surfaces
        {
            return Ok(None);
        }
        capabilities.push(project_extension(&extension, surfaces).await?);
    }
    Ok(Some(capabilities))
}

#[cfg(feature = "extensions")]
async fn project_extension(
    extension: &a3s_use_extension::InstalledExtension,
    surfaces: Vec<String>,
) -> UseResult<CapabilityBinding> {
    let paths = a3s_use_extension::ExtensionPaths::from_env()?;
    let scope = default_plan_scope();
    let flow_observations = flow_observations_from_store(
        extension,
        &FlowRuntimeBindingStore::from_extension_paths(&paths),
        &scope,
    )
    .await?;
    let knowledge_evidence = knowledge_evidence_from_store(
        extension,
        &OkfKnowledgeBindingStore::from_extension_paths(&paths),
        &OkfKnowledgeClient::new(std::sync::Arc::new(
            SqliteOkfKnowledgeAdapter::from_extension_paths(&paths),
        )),
        &scope,
    )
    .await?;
    let runtime_task_evidence = runtime_task_evidence_from_store(
        extension,
        &RuntimeBindingStore::from_extension_paths(&paths),
        &scope,
    )
    .await?;
    let mut host_observations = flow_observations;
    for (surface, state) in runtime_task_evidence
        .observations
        .into_iter()
        .chain(knowledge_evidence.failures)
    {
        if host_observations.insert(surface, state).is_some() {
            return Err(UseError::new(
                "use.capability.host_observation_invalid",
                "Two production hosts reported the same cognitive-package surface.",
            ));
        }
    }
    project_extension_for_host_with_flow_observations(
        extension,
        surfaces,
        env!("CARGO_PKG_VERSION"),
        &host_observations,
        &knowledge_evidence.bindings,
        &runtime_task_evidence.projections,
    )
    .await
}

#[cfg(feature = "extensions")]
#[cfg(test)]
async fn project_extension_for_host(
    extension: &a3s_use_extension::InstalledExtension,
    surfaces: Vec<String>,
    host_version: &str,
) -> UseResult<CapabilityBinding> {
    project_extension_for_host_with_flow_observations(
        extension,
        surfaces,
        host_version,
        &SurfaceObservations::new(),
        &[],
        &[],
    )
    .await
}

#[cfg(feature = "extensions")]
async fn project_extension_for_host_with_flow_observations(
    extension: &a3s_use_extension::InstalledExtension,
    surfaces: Vec<String>,
    host_version: &str,
    host_observations: &SurfaceObservations,
    knowledge_bindings: &[OkfKnowledgeBinding],
    runtime_tasks: &[ToolTaskProjection],
) -> UseResult<CapabilityBinding> {
    let receipt = &extension.receipt;
    let compatible = extension.supports_use_version(host_version);
    let observations =
        surface_observations(extension, receipt.enabled && compatible, host_observations).await?;
    let knowledge_observations = knowledge_bindings
        .iter()
        .map(|binding| (binding.receipt.clone(), binding.observation.clone()))
        .collect::<Vec<_>>();
    let reconciliation = Some(reconcile_with_runtime_and_knowledge(
        &extension.manifest,
        if receipt.enabled {
            PluginDesiredState::Enabled
        } else {
            PluginDesiredState::InstalledDisabled
        },
        compatible,
        &observations,
        None,
        &knowledge_observations,
    )?);
    let active = receipt.enabled
        && compatible
        && reconciliation
            .as_ref()
            .is_some_and(|snapshot| snapshot.capability_ready);
    let readiness = match reconciliation
        .as_ref()
        .expect("reconciliation is present")
        .observed
    {
        PluginObservedState::Ready | PluginObservedState::Degraded => Readiness::Ready,
        PluginObservedState::Broken | PluginObservedState::Incompatible => Readiness::Broken,
        PluginObservedState::Installed
        | PluginObservedState::Reconciling
        | PluginObservedState::Draining
        | PluginObservedState::Removed => Readiness::Unknown,
    };
    let mcp = None;
    let mut skills = Vec::new();
    if active {
        let snapshot = reconciliation.as_ref().expect("reconciliation is present");
        for skill in &extension.manifest.skills {
            if snapshot.publishes(PluginSurfaceKind::Skill, &skill.id) {
                skills.push(skill_surface(receipt.package_root.join(&skill.path)).await?);
            }
        }
    }
    let mut activity_bar = Vec::new();
    let mut flows = Vec::new();
    let mut knowledge = Vec::new();
    let mut tool_tasks = Vec::new();
    if let Some(snapshot) = reconciliation.as_ref().filter(|_| active) {
        tool_tasks.extend(
            runtime_tasks
                .iter()
                .filter(|task| snapshot.publishes(PluginSurfaceKind::Tool, &task.surface_id))
                .cloned(),
        );
    }
    if let Some(snapshot) = reconciliation.as_ref().filter(|_| active) {
        for surface in &extension.manifest.flows {
            if !snapshot.publishes(PluginSurfaceKind::Flow, &surface.id) {
                continue;
            }
            flows.push(FlowSurface {
                id: surface.id.clone(),
                engine: match surface.engine {
                    a3s_use_extension::PluginFlowEngine::A3sFlow => FlowEngine::A3sFlow,
                },
                runtime: match surface.runtime {
                    a3s_use_extension::PluginFlowRuntime::NativeTs => FlowRuntime::NativeTs,
                },
                source: flow_asset(receipt.package_root.join(&surface.source)).await?,
                export_name: surface.export_name.clone(),
                requires_tools: surface.requires_tools.clone(),
                requires_mcp: surface.requires_mcp.clone(),
                requires_okf: surface.requires_okf.clone(),
            });
        }
    }
    if let Some(snapshot) = reconciliation.as_ref().filter(|_| active) {
        for binding in knowledge_bindings {
            if snapshot.publishes(PluginSurfaceKind::Okf, &binding.receipt.surface.surface.id) {
                knowledge.push(OkfCapabilityProjection::from_promoted(
                    &binding.receipt,
                    &binding.observation,
                )?);
            }
        }
        knowledge.sort_by(|left, right| left.surface.cmp(&right.surface));
        for surface in &extension.manifest.ui {
            if !snapshot.publishes(PluginSurfaceKind::Ui, &surface.id) {
                continue;
            }
            let mut styles = Vec::with_capacity(surface.styles.len());
            for path in &surface.styles {
                styles.push(activity_asset(receipt.package_root.join(path), "text/css").await?);
            }
            let mut scripts = Vec::with_capacity(surface.scripts.len());
            for path in &surface.scripts {
                scripts.push(
                    activity_asset(receipt.package_root.join(path), "text/javascript").await?,
                );
            }
            activity_bar.push(ActivityBarContribution {
                id: surface.id.clone(),
                title: surface.title.clone(),
                description: surface.description.clone(),
                icon: surface.icon.clone(),
                entry: activity_asset(receipt.package_root.join(&surface.entry), "text/html")
                    .await?,
                styles,
                scripts,
                skill: surface.skill.clone(),
                order: surface.order,
            });
        }
    }
    let planner_evidence = plugin_planner_evidence(extension, reconciliation.as_ref())?;
    Ok(CapabilityBinding {
        id: receipt.component_id.clone(),
        route: receipt.route.clone(),
        version: receipt.version.clone(),
        origin: CapabilityOrigin::Extension,
        enabled: active,
        readiness,
        reconciliation,
        planner_evidence,
        package_root: Some(receipt.package_root.clone()),
        lifecycle_generation: receipt.lifecycle_generation,
        requires_use: extension.manifest.requires_use.clone(),
        repository: extension
            .manifest
            .repository
            .as_ref()
            .map(|repository| RepositoryBinding {
                url: repository.url.clone(),
                revision: repository.revision.clone(),
            }),
        surfaces,
        mcp,
        skills,
        flows,
        knowledge,
        activity_bar,
        tool_tasks,
    })
}

#[cfg(feature = "extensions")]
async fn surface_observations(
    extension: &a3s_use_extension::InstalledExtension,
    inspect_enabled_surfaces: bool,
    host_observations: &SurfaceObservations,
) -> UseResult<SurfaceObservations> {
    if host_observations.keys().any(|surface| match surface.kind {
        PluginSurfaceKind::Flow => !extension
            .manifest
            .flows
            .iter()
            .any(|flow| flow.id == surface.id),
        PluginSurfaceKind::Tool => !extension.manifest.tools.iter().any(|tool| {
            tool.id == surface.id
                && matches!(
                    &tool.workload,
                    a3s_use_extension::ToolWorkload::Task(task)
                        if matches!(&task.source, a3s_use_extension::ToolTaskSource::Release { .. })
                            && !task.interactive
                )
        }),
        PluginSurfaceKind::Okf => !extension
            .manifest
            .okf
            .iter()
            .any(|okf| okf.id == surface.id),
        _ => true,
    }) {
        return Err(UseError::new(
            "use.capability.host_observation_invalid",
            "Production host observations must reference only their admitted Flow or OKF surfaces.",
        ));
    }
    if !inspect_enabled_surfaces {
        return Ok(SurfaceObservations::new());
    }

    let mut observations = host_observations.clone();
    for surface in &extension.manifest.flows {
        if a3s_use_extension::inspect_flow_surface_file(surface, &extension.receipt.package_root)
            .await
            .is_err()
        {
            observations.insert(
                PluginSurfaceRef {
                    kind: PluginSurfaceKind::Flow,
                    id: surface.id.clone(),
                },
                SurfaceObservedState::Failed,
            );
        }
    }
    for surface in &extension.manifest.skills {
        let observed = match a3s_use_extension::inspect_skill_surface_file(
            surface,
            &extension.receipt.package_root,
        )
        .await
        {
            Ok(_) => SurfaceObservedState::Prepared,
            Err(_) => SurfaceObservedState::Failed,
        };
        observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Skill,
                id: surface.id.clone(),
            },
            observed,
        );
    }
    for surface in &extension.manifest.ui {
        let observed = match a3s_use_extension::inspect_ui_surface_files(
            surface,
            &extension.receipt.package_root,
        )
        .await
        {
            Ok(_) => SurfaceObservedState::Prepared,
            Err(_) => SurfaceObservedState::Failed,
        };
        observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Ui,
                id: surface.id.clone(),
            },
            observed,
        );
    }
    Ok(observations)
}

#[cfg(feature = "extensions")]
async fn flow_observations_from_store(
    extension: &a3s_use_extension::InstalledExtension,
    store: &FlowRuntimeBindingStore,
    scope: &a3s_use_core::PlanScope,
) -> UseResult<SurfaceObservations> {
    let mut observations = SurfaceObservations::new();
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(observations);
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(observations);
    };
    let package_digest = format!("sha256:{package_sha256}");
    let manifest_digest = format!("sha256:{}", extension.receipt.manifest_sha256);
    for surface in &extension.manifest.flows {
        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: surface.id.clone(),
        };
        let qualified = PlanQualifiedSurfaceRef {
            package_id: extension.receipt.package_id.clone(),
            surface: reference.clone(),
        };
        let Some(binding) = store.get(scope, &qualified, generation).await? else {
            continue;
        };
        let state = if binding.package_digest() == package_digest
            && binding.manifest_digest() == manifest_digest
            && binding
                .inspect(surface, &extension.receipt.package_root)
                .await
                .is_ok()
        {
            SurfaceObservedState::Prepared
        } else {
            SurfaceObservedState::Failed
        };
        observations.insert(reference, state);
    }
    Ok(observations)
}

#[cfg(feature = "extensions")]
struct KnowledgeEvidence {
    bindings: Vec<OkfKnowledgeBinding>,
    failures: SurfaceObservations,
}

#[cfg(feature = "extensions")]
async fn knowledge_evidence_from_store(
    extension: &a3s_use_extension::InstalledExtension,
    store: &OkfKnowledgeBindingStore,
    client: &OkfKnowledgeClient,
    scope: &a3s_use_core::PlanScope,
) -> UseResult<KnowledgeEvidence> {
    let mut bindings = Vec::new();
    let mut failures = SurfaceObservations::new();
    let Some(generation) = extension.receipt.lifecycle_generation else {
        return Ok(KnowledgeEvidence { bindings, failures });
    };
    let Some(package_sha256) = extension.receipt.package_sha256.as_deref() else {
        return Ok(KnowledgeEvidence { bindings, failures });
    };
    let package_digest = format!("sha256:{package_sha256}");
    let manifest_digest = format!("sha256:{}", extension.receipt.manifest_sha256);

    for surface in &extension.manifest.okf {
        let reference = PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: surface.id.clone(),
        };
        let qualified = PlanQualifiedSurfaceRef {
            package_id: extension.receipt.package_id.clone(),
            surface: reference.clone(),
        };
        let Some(binding) = store.get(scope, &qualified, generation).await? else {
            continue;
        };
        let exact = binding.receipt.scope == *scope
            && binding.receipt.surface == qualified
            && binding.receipt.generation == generation
            && binding.receipt.package_digest == package_digest
            && binding.receipt.manifest_digest == manifest_digest
            && binding.receipt.bundle == surface.bundle;
        if !exact {
            failures.insert(reference, SurfaceObservedState::Failed);
            continue;
        }
        match client.observe(&binding.receipt).await {
            Ok(observed) if observed == binding => bindings.push(observed),
            Ok(_) | Err(_) => {
                failures.insert(reference, SurfaceObservedState::Failed);
            }
        }
    }
    Ok(KnowledgeEvidence { bindings, failures })
}

#[cfg(feature = "extensions")]
fn default_plan_scope() -> a3s_use_core::PlanScope {
    a3s_use_core::PlanScope {
        kind: a3s_use_core::PlanScopeKind::User,
        id: COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_owned(),
    }
}

#[cfg(feature = "extensions")]
fn plugin_planner_evidence(
    extension: &a3s_use_extension::InstalledExtension,
    reconciliation: Option<&SurfaceReconcileSnapshot>,
) -> UseResult<Option<PluginPlannerEvidence>> {
    if extension.manifest.schema_version != 3 {
        return Ok(None);
    }
    let catalog = match extension.plan_ready_catalog() {
        Ok(catalog) => catalog,
        Err(error) if error.code == "use.extension.plan_evidence_missing" => return Ok(None),
        Err(error) => return Err(error),
    };
    let reconciliation = reconciliation.ok_or_else(|| {
        UseError::new(
            "use.capability.planner_evidence_invalid",
            "A plan-ready schema-v3 plugin omitted reconciliation evidence.",
        )
    })?;
    let mut selected_surfaces = reconciliation
        .surfaces
        .iter()
        .map(|surface| surface.surface.clone())
        .collect::<Vec<_>>();
    selected_surfaces.sort();
    selected_surfaces.dedup();
    let catalog_surfaces = catalog
        .record
        .surfaces
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    if selected_surfaces.is_empty() || selected_surfaces != catalog_surfaces {
        return Err(UseError::new(
            "use.capability.planner_evidence_invalid",
            "The installed manifest surface inventory does not match its verified catalog.",
        ));
    }
    let planned = catalog.selected_state(&selected_surfaces)?;
    let planned_surfaces = planned
        .release
        .surfaces
        .iter()
        .map(|surface| surface.reference())
        .collect::<Vec<_>>();
    if planned_surfaces != selected_surfaces {
        return Err(UseError::new(
            "use.capability.planner_evidence_invalid",
            "The capability surface selection is not closed under catalog dependencies.",
        ));
    }
    let package_sha256 = extension.receipt.package_sha256.as_deref().ok_or_else(|| {
        UseError::new(
            "use.capability.planner_evidence_invalid",
            "A plan-ready receipt omitted its expanded-package digest.",
        )
    })?;
    Ok(Some(PluginPlannerEvidence {
        schema_version: PLANNER_EVIDENCE_SCHEMA_VERSION,
        package_id: extension.receipt.package_id.clone(),
        package_sha256: format!("sha256:{package_sha256}"),
        manifest_sha256: format!("sha256:{}", extension.receipt.manifest_sha256),
        receipt_digest: extension.receipt.descriptor_digest()?,
        catalog_record_digest: catalog.provenance.catalog_record_digest.clone(),
        desired_enabled: extension.receipt.enabled,
        selected_surfaces,
    }))
}

#[cfg(feature = "extensions")]
fn installed_plugin_plan_evidence_from_snapshot(
    snapshot: &CapabilityRegistrySnapshot,
    extension: &a3s_use_extension::InstalledExtension,
) -> UseResult<InstalledPluginPlanEvidence> {
    let receipt = &extension.receipt;
    let binding = snapshot
        .capabilities
        .iter()
        .find(|binding| binding.id == receipt.component_id)
        .ok_or_else(|| {
            UseError::new(
                "use.capability.planner_evidence_missing",
                "The installed package is absent from the stable capability snapshot.",
            )
        })?;
    let summary = binding.planner_evidence.as_ref().ok_or_else(|| {
        UseError::new(
            "use.capability.planner_evidence_missing",
            "The installed package does not expose plan-ready capability evidence.",
        )
    })?;
    let catalog = extension.plan_ready_catalog()?.clone();
    let receipt_digest = receipt.descriptor_digest()?;
    let package_sha256 = catalog.record.package.sha256.as_deref();
    let manifest_sha256 = catalog.record.package.manifest_sha256.as_deref();
    if binding.origin != CapabilityOrigin::Extension
        || binding.version != receipt.version
        || summary.package_id != receipt.package_id
        || summary.package_sha256.as_str() != package_sha256.unwrap_or_default()
        || summary.manifest_sha256.as_str() != manifest_sha256.unwrap_or_default()
        || summary.receipt_digest != receipt_digest
        || summary.catalog_record_digest != catalog.provenance.catalog_record_digest
        || summary.desired_enabled != receipt.enabled
    {
        return Err(UseError::new(
            "use.capability.planner_evidence_invalid",
            "The package-specific receipt evidence does not match the stable capability snapshot.",
        ));
    }
    let evidence = InstalledPluginPlanEvidence {
        schema: INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA.to_owned(),
        component_id: receipt.component_id.clone(),
        package_id: receipt.package_id.clone(),
        version: receipt.version.clone(),
        capability_generation: snapshot.generation,
        capability_revision: snapshot.revision.clone(),
        receipt_digest,
        desired_enabled: summary.desired_enabled,
        selected_surfaces: summary.selected_surfaces.clone(),
        verified_catalog: catalog,
    };
    evidence.validate()?;
    Ok(evidence)
}

#[cfg(all(test, feature = "extensions"))]
#[path = "capability_registry_planner_tests.rs"]
mod planner_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "extensions")]
    const SKILL_ONLY_PLUGIN: &str = r#"
extension "acme/guide" {
  schema_version = 3
  version        = "1.0.0"
  route          = "guide"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/guide"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  skill "guide" {
    path          = "skills/guide/SKILL.md"
    requires_tool = []
    requires_mcp  = []
    optional      = false
  }
}
"#;

    #[cfg(feature = "extensions")]
    const SKILL_UI_PLUGIN: &str = r#"
extension "acme/workbench" {
  schema_version = 3
  version        = "1.0.0"
  route          = "workbench"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/workbench"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  skill "guide" {
    path          = "skills/guide/SKILL.md"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }

  ui "review" {
    title       = "Evidence Review"
    description = "Review the cognitive package evidence."
    icon        = "flask-conical"
    order       = 80
    entry        = "ui/review.html"
    styles       = ["ui/review.css"]
    scripts      = ["ui/review.js"]
    skill        = "guide"
    bind_tool    = []
    bind_mcp     = []
    optional     = false
  }

  ui "standalone" {
    entry     = "ui/standalone.html"
    styles    = []
    scripts   = []
    bind_tool = []
    bind_mcp  = []
    optional  = false
  }
}
"#;

    #[cfg(feature = "extensions")]
    const FLOW_PLUGIN: &str = r#"
extension "acme/workflow" {
  schema_version = 3
  version        = "1.0.0"
  route          = "workflow"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/workflow"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  flow "review" {
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/review.ts"
    export        = "run"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }
}
"#;

    #[cfg(feature = "extensions")]
    fn installed_extension(
        manifest: a3s_use_extension::ExtensionManifest,
        package_root: PathBuf,
        enabled: bool,
    ) -> a3s_use_extension::InstalledExtension {
        let mut selected_surfaces = manifest
            .plugin_surfaces()
            .unwrap()
            .into_iter()
            .map(|surface| surface.surface)
            .collect::<Vec<_>>();
        selected_surfaces.sort();
        let receipt = a3s_use_extension::ExtensionReceipt {
            schema_version: 4,
            package_id: manifest.package_id.clone(),
            component_id: format!("use/{}", manifest.route),
            route: manifest.route.clone(),
            version: manifest.version.clone(),
            package_root,
            manifest_sha256: "0".repeat(64),
            package_sha256: Some("0".repeat(64)),
            trust: a3s_use_extension::ExtensionTrust::LocalExplicit,
            registry: None,
            verified_catalog: None,
            planning_bundle: None,
            selected_surfaces,
            installed_at_unix: 0,
            enabled,
            lifecycle_generation: Some(1),
        };
        a3s_use_extension::InstalledExtension { receipt, manifest }
    }

    #[tokio::test]
    async fn built_ins_are_projected_without_extension_identity() {
        let snapshot = snapshot().await.unwrap();
        assert_eq!(snapshot.schema_version, 2);
        let browser = snapshot
            .capabilities
            .iter()
            .find(|capability| capability.id == "use/browser")
            .unwrap();
        let ocr = snapshot
            .capabilities
            .iter()
            .find(|capability| capability.id == "use/ocr")
            .unwrap();

        assert_eq!(browser.origin, CapabilityOrigin::BuiltIn);
        assert_eq!(ocr.origin, CapabilityOrigin::BuiltIn);
        assert!(snapshot
            .capabilities
            .iter()
            .filter(|capability| capability.route == "office")
            .all(|capability| capability.origin == CapabilityOrigin::Extension));
        #[cfg(feature = "browser")]
        {
            assert!(browser.surfaces.iter().any(|surface| surface == "skill"));
            assert!(browser
                .skills
                .iter()
                .any(|skill| skill.path.ends_with("a3s-use-browser/SKILL.md")));
            assert!(browser.skills.iter().all(|skill| skill.sha256.len() == 64));
        }
        #[cfg(not(feature = "browser"))]
        {
            assert!(!browser.enabled);
            assert!(browser.surfaces.is_empty());
            assert!(browser.skills.is_empty());
        }
        #[cfg(feature = "ocr")]
        {
            assert!(ocr.enabled);
            assert!(ocr.surfaces.iter().any(|surface| surface == "skill"));
            assert!(ocr
                .skills
                .iter()
                .any(|skill| skill.path.ends_with("a3s-use-ocr/SKILL.md")));
            assert!(ocr.skills.iter().all(|skill| skill.sha256.len() == 64));
            #[cfg(feature = "mcp")]
            assert_eq!(
                ocr.mcp.as_ref().map(|surface| surface.target.as_str()),
                Some("ocr-native")
            );
        }
        #[cfg(not(feature = "ocr"))]
        {
            assert!(!ocr.enabled);
            assert!(ocr.surfaces.is_empty());
            assert!(ocr.skills.is_empty());
        }
        assert_eq!(snapshot.revision.len(), 64);
    }

    #[tokio::test]
    async fn skill_content_changes_revision_without_changing_its_path() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("SKILL.md");
        tokio::fs::write(&path, b"first").await.unwrap();
        let first = skill_surface(path.clone()).await.unwrap();
        tokio::fs::write(&path, b"second").await.unwrap();
        let second = skill_surface(path).await.unwrap();
        assert_ne!(first.sha256, second.sha256);

        let mut capability = box_capability();
        capability.skills = vec![first];
        let first_revision = revision(&[capability.clone()]).unwrap();
        capability.skills = vec![second];
        let second_revision = revision(&[capability]).unwrap();
        assert_ne!(first_revision, second_revision);
    }

    #[tokio::test]
    async fn activity_asset_content_is_integrity_bound_to_the_registry_revision() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("activity.html");
        tokio::fs::write(&path, b"<main>first</main>")
            .await
            .unwrap();
        let first = activity_asset(path.clone(), "text/html").await.unwrap();
        tokio::fs::write(&path, b"<main>second</main>")
            .await
            .unwrap();
        let second = activity_asset(path, "text/html").await.unwrap();
        assert_ne!(first.sha256, second.sha256);

        let mut capability = box_capability();
        capability.activity_bar = vec![ActivityBarContribution {
            id: "science".to_string(),
            title: "Science".to_string(),
            description: "Scientific workspace".to_string(),
            icon: "flask-conical".to_string(),
            entry: first,
            styles: Vec::new(),
            scripts: Vec::new(),
            skill: Some("science".to_string()),
            order: 120,
        }];
        let first_revision = revision(&[capability.clone()]).unwrap();
        capability.activity_bar[0].entry = second;
        let second_revision = revision(&[capability]).unwrap();
        assert_ne!(first_revision, second_revision);
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn promoted_sqlite_knowledge_enters_the_scope_aware_capability_projection() {
        const MANIFEST: &str = include_str!(
            "../crates/extension/fixtures/packages/plugin-v3-okf/package/a3s-use-extension.acl"
        );
        const PACKAGE_DIGEST: &str =
            include_str!("../crates/extension/fixtures/packages/plugin-v3-okf/package.sha256");
        let temporary = tempfile::tempdir().unwrap();
        let package_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(MANIFEST).unwrap();
        let mut extension = installed_extension(manifest.clone(), package_root.clone(), true);
        let package_digest = PACKAGE_DIGEST.trim().to_owned();
        extension.receipt.package_sha256 =
            Some(package_digest.strip_prefix("sha256:").unwrap().to_owned());
        extension.receipt.manifest_sha256 = format!("{:x}", Sha256::digest(MANIFEST.as_bytes()));
        extension.receipt.lifecycle_generation = Some(7);
        let paths = a3s_use_extension::ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
        );
        let adapter = std::sync::Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(&paths));
        let client = OkfKnowledgeClient::new(adapter);
        let store = OkfKnowledgeBindingStore::from_extension_paths(&paths);
        let surface = &manifest.okf[0];
        let files = a3s_use_extension::load_okf_bundle_files(surface, &package_root)
            .await
            .unwrap();
        let scope = default_plan_scope();
        let staged = client
            .stage(
                crate::okf_knowledge::OkfKnowledgeStageRequest::new(
                    crate::okf_knowledge::OkfKnowledgeStageSpec {
                        operation_id: "capability-knowledge-stage".to_owned(),
                        scope: scope.clone(),
                        surface: PlanQualifiedSurfaceRef {
                            package_id: manifest.package_id.clone(),
                            surface: PluginSurfaceRef {
                                kind: PluginSurfaceKind::Okf,
                                id: surface.id.clone(),
                            },
                        },
                        generation: 7,
                        package_digest: package_digest.clone(),
                        manifest_digest: format!("sha256:{}", extension.receipt.manifest_sha256),
                        bundle: surface.bundle.clone(),
                    },
                    files,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        store.put(&staged).await.unwrap();
        let promoted = client.promote(&staged.receipt).await.unwrap();
        store.put(&promoted).await.unwrap();

        let evidence = knowledge_evidence_from_store(&extension, &store, &client, &scope)
            .await
            .unwrap();
        assert!(evidence.failures.is_empty());
        let binding = project_extension_for_host_with_flow_observations(
            &extension,
            extension
                .surfaces()
                .into_iter()
                .map(str::to_owned)
                .collect(),
            "0.3.0",
            &SurfaceObservations::new(),
            &evidence.bindings,
            &[],
        )
        .await
        .unwrap();
        assert!(binding.enabled);
        assert_eq!(binding.knowledge.len(), 1);
        assert_eq!(binding.knowledge[0].scope, scope);
        assert_eq!(binding.knowledge[0].generation, 7);

        let response = client
            .search(
                &crate::okf_knowledge::OkfKnowledgeSearchRequest::new(
                    scope,
                    "package activation",
                    5,
                    binding.knowledge,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            response.hits[0].citation.path,
            "concepts/package-lifecycle.md"
        );
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn schema_three_projects_only_dependency_ready_named_skills() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("skills").join("guide").join("SKILL.md");
        tokio::fs::create_dir_all(path.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&path, b"# Guide\n").await.unwrap();
        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_ONLY_PLUGIN).unwrap();
        let mut extension = installed_extension(manifest, temp.path().to_path_buf(), true);
        extension.receipt.package_sha256 = Some("a".repeat(64));
        extension.receipt.lifecycle_generation = Some(7);
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
            .await
            .unwrap();
        let reconciliation = binding.reconciliation.as_ref().unwrap();

        assert!(binding.enabled);
        assert_eq!(binding.readiness, Readiness::Ready);
        assert_eq!(binding.skills.len(), 1);
        assert_eq!(binding.skills[0].path, path);
        assert_eq!(binding.skills[0].sha256.len(), 64);
        assert_eq!(binding.lifecycle_generation, Some(7));
        assert_eq!(reconciliation.observed, PluginObservedState::Ready);
        assert!(reconciliation.publishes(PluginSurfaceKind::Skill, "guide"));

        let json = serde_json::to_value(&binding).unwrap();
        assert_eq!(json["reconciliation"]["desired"], "enabled");
        assert_eq!(json["reconciliation"]["observed"], "ready");
        assert_eq!(json["lifecycleGeneration"], 7);
        assert_eq!(
            json["reconciliation"]["surfaces"][0]["surface"]["id"],
            "guide"
        );
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn schema_three_projects_ready_ui_assets_with_optional_skill_guidance() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("skills/guide/SKILL.md");
        let review = temp.path().join("ui/review.html");
        let style = temp.path().join("ui/review.css");
        let script = temp.path().join("ui/review.js");
        let standalone = temp.path().join("ui/standalone.html");
        tokio::fs::create_dir_all(skill.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::create_dir_all(review.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&skill, b"# Guide\n").await.unwrap();
        tokio::fs::write(&review, b"<main>review</main>")
            .await
            .unwrap();
        tokio::fs::write(&style, b"main { color: purple; }")
            .await
            .unwrap();
        tokio::fs::write(&script, b"window.reviewReady = true;")
            .await
            .unwrap();
        tokio::fs::write(&standalone, b"<main>standalone</main>")
            .await
            .unwrap();
        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_UI_PLUGIN).unwrap();
        let mut extension = installed_extension(manifest, temp.path().to_path_buf(), true);
        extension.receipt.package_sha256 = Some("a".repeat(64));
        extension.receipt.lifecycle_generation = Some(9);
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect();

        let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
            .await
            .unwrap();
        assert_eq!(binding.activity_bar.len(), 2);
        let review = binding
            .activity_bar
            .iter()
            .find(|activity| activity.id == "review")
            .unwrap();
        assert_eq!(review.title, "Evidence Review");
        assert_eq!(review.description, "Review the cognitive package evidence.");
        assert_eq!(review.icon, "flask-conical");
        assert_eq!(review.order, 80);
        assert_eq!(review.skill.as_deref(), Some("guide"));
        assert_eq!(review.entry.media_type, "text/html");
        assert_eq!(review.styles[0].media_type, "text/css");
        assert_eq!(review.scripts[0].media_type, "text/javascript");

        let standalone = binding
            .activity_bar
            .iter()
            .find(|activity| activity.id == "standalone")
            .unwrap();
        assert_eq!(standalone.title, "standalone");
        assert_eq!(standalone.icon, "package");
        assert_eq!(standalone.order, 100);
        assert!(standalone.skill.is_none());
        assert!(binding
            .reconciliation
            .as_ref()
            .unwrap()
            .publishes(PluginSurfaceKind::Ui, "review"));
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn schema_three_requires_a3s_flow_host_preflight_before_catalog_publication() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("flows/review.ts");
        tokio::fs::create_dir_all(source.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &source,
            b"export async function run() { return { type: 'complete', output: null }; }\n",
        )
        .await
        .unwrap();
        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(FLOW_PLUGIN).unwrap();
        let extension = installed_extension(manifest, temp.path().to_path_buf(), true);
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect::<Vec<_>>();

        let source_only = project_extension_for_host(&extension, surfaces.clone(), "0.3.0")
            .await
            .unwrap();
        let source_only_flow = source_only
            .reconciliation
            .as_ref()
            .unwrap()
            .surfaces
            .iter()
            .find(|surface| surface.surface.kind == PluginSurfaceKind::Flow)
            .unwrap();
        assert!(!source_only.enabled);
        assert_eq!(source_only.readiness, Readiness::Unknown);
        assert!(source_only.flows.is_empty());
        assert_eq!(source_only_flow.observed, SurfaceObservedState::Pending);
        assert_eq!(
            source_only_flow.reason,
            Some(crate::surface_reconciler::SurfaceStateReason::FlowObservationMissing)
        );

        let mut flow_observations = SurfaceObservations::new();
        flow_observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "review".to_owned(),
            },
            SurfaceObservedState::Prepared,
        );
        let binding = project_extension_for_host_with_flow_observations(
            &extension,
            surfaces,
            "0.3.0",
            &flow_observations,
            &[],
            &[],
        )
        .await
        .unwrap();
        assert!(binding.enabled);
        assert_eq!(binding.readiness, Readiness::Ready);
        assert_eq!(binding.flows.len(), 1);
        let flow = &binding.flows[0];
        assert_eq!(flow.id, "review");
        assert_eq!(flow.engine, FlowEngine::A3sFlow);
        assert_eq!(flow.runtime, FlowRuntime::NativeTs);
        assert_eq!(flow.source.path, source);
        assert_eq!(flow.source.sha256.len(), 64);
        assert_eq!(flow.source.media_type, "text/typescript");
        assert_eq!(flow.export_name, "run");
        assert!(binding
            .reconciliation
            .as_ref()
            .unwrap()
            .publishes(PluginSurfaceKind::Flow, "review"));

        let json = serde_json::to_value(&binding).unwrap();
        assert_eq!(json["flows"][0]["engine"], "a3s-flow");
        assert_eq!(json["flows"][0]["runtime"], "native-ts");
        assert_eq!(json["flows"][0]["exportName"], "run");
        assert_eq!(json["flows"][0]["source"]["mediaType"], "text/typescript");
    }

    #[cfg(all(feature = "extensions", unix))]
    #[tokio::test]
    async fn exact_generation_flow_binding_drives_production_observation() {
        use std::os::unix::fs::PermissionsExt;

        use crate::flow_runtime::{A3sFlowLifecycleHost, FlowRuntimeBindingStore};
        use crate::plugin_lifecycle::{
            PluginFlowLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
            PluginLifecycleIntentSpec,
        };

        let temp = tempfile::tempdir().unwrap();
        let package_root = temp.path().join("package");
        let source = package_root.join("flows/review.ts");
        tokio::fs::create_dir_all(source.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(
            &source,
            b"export async function run() { return { type: 'complete', output: null }; }\n",
        )
        .await
        .unwrap();
        let compiler = temp.path().join("a3s-flow-native-compiler");
        tokio::fs::write(
            &compiler,
            b"#!/bin/sh\nset -eu\nwhile [ \"$1\" != \"-o\" ]; do shift; done\nshift\nprintf '#!/bin/sh\\nexit 0\\n' > \"$1\"\nchmod +x \"$1\"\n",
        )
        .await
        .unwrap();
        let mut permissions = std::fs::metadata(&compiler).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&compiler, permissions).unwrap();

        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(FLOW_PLUGIN).unwrap();
        let mut extension = installed_extension(manifest, package_root.clone(), true);
        extension.receipt.package_sha256 = Some("a".repeat(64));
        extension.receipt.lifecycle_generation = Some(12);
        let intent = PluginLifecycleIntent::from_manifest(
            PluginLifecycleIntentSpec {
                operation_id: "flow-observation-install".to_string(),
                plan_digest: format!("sha256:{}", "1".repeat(64)),
                scope: a3s_use_core::PlanScope {
                    kind: a3s_use_core::PlanScopeKind::User,
                    id: COGNITIVE_PACKAGE_DEFAULT_SCOPE.to_string(),
                },
                package_id: extension.receipt.package_id.clone(),
                package_digest: format!("sha256:{}", "a".repeat(64)),
                manifest_digest: format!("sha256:{}", extension.receipt.manifest_sha256),
                generation: 12,
                action: PluginLifecycleAction::Install,
                retained_ui_state_surfaces: Vec::new(),
            },
            &extension.manifest,
        )
        .unwrap();
        let key = &intent
            .checkpoints
            .iter()
            .find(|checkpoint| {
                checkpoint
                    .surface
                    .as_ref()
                    .is_some_and(|surface| surface.kind == PluginSurfaceKind::Flow)
            })
            .unwrap()
            .idempotency_key;
        let store = FlowRuntimeBindingStore::new(temp.path().join("state"));
        let host = A3sFlowLifecycleHost::new(
            &package_root,
            &compiler,
            temp.path().join("cache"),
            store.clone(),
        );
        host.prepare_flow(&intent, &extension.manifest.flows[0], key)
            .await
            .unwrap();

        let observations = flow_observations_from_store(&extension, &store, &default_plan_scope())
            .await
            .unwrap();
        assert_eq!(
            observations.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "review".to_string(),
            }),
            Some(&SurfaceObservedState::Prepared)
        );
        let binding = store
            .get(
                &default_plan_scope(),
                &PlanQualifiedSurfaceRef {
                    package_id: extension.receipt.package_id.clone(),
                    surface: PluginSurfaceRef {
                        kind: PluginSurfaceKind::Flow,
                        id: "review".to_string(),
                    },
                },
                12,
            )
            .await
            .unwrap()
            .unwrap();
        tokio::fs::write(binding.artifact(), b"substituted")
            .await
            .unwrap();
        let failed = flow_observations_from_store(&extension, &store, &default_plan_scope())
            .await
            .unwrap();
        assert_eq!(
            failed.get(&PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "review".to_string(),
            }),
            Some(&SurfaceObservedState::Failed)
        );
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn required_a3s_flow_source_corruption_withholds_the_generation() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("flows/review.ts");
        tokio::fs::create_dir_all(source.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&source, [0xff_u8]).await.unwrap();
        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(FLOW_PLUGIN).unwrap();
        let extension = installed_extension(manifest, temp.path().to_path_buf(), true);
        let mut flow_observations = SurfaceObservations::new();
        flow_observations.insert(
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Flow,
                id: "review".to_owned(),
            },
            SurfaceObservedState::Prepared,
        );
        let binding = project_extension_for_host_with_flow_observations(
            &extension,
            extension
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            "0.3.0",
            &flow_observations,
            &[],
            &[],
        )
        .await
        .unwrap();
        let flow = binding
            .reconciliation
            .as_ref()
            .unwrap()
            .surfaces
            .iter()
            .find(|surface| surface.surface.kind == PluginSurfaceKind::Flow)
            .unwrap();

        assert!(!binding.enabled);
        assert_eq!(binding.readiness, Readiness::Broken);
        assert!(binding.flows.is_empty());
        assert_eq!(flow.observed, SurfaceObservedState::Failed);
        assert!(!flow.published);
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn schema_three_ui_integrity_failure_blocks_only_required_surfaces() {
        let temp = tempfile::tempdir().unwrap();
        let skill = temp.path().join("skills/guide/SKILL.md");
        let review = temp.path().join("ui/review.html");
        tokio::fs::create_dir_all(skill.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::create_dir_all(review.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(&skill, b"# Guide\n").await.unwrap();
        tokio::fs::write(&review, b"<main>review</main>")
            .await
            .unwrap();
        tokio::fs::write(
            temp.path().join("ui/review.css"),
            b"main { color: purple; }",
        )
        .await
        .unwrap();
        tokio::fs::write(
            temp.path().join("ui/review.js"),
            b"window.reviewReady = true;",
        )
        .await
        .unwrap();

        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(SKILL_UI_PLUGIN).unwrap();
        let required = installed_extension(manifest.clone(), temp.path().to_path_buf(), true);
        let binding = project_extension_for_host(
            &required,
            required
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            "0.3.0",
        )
        .await
        .unwrap();
        assert!(!binding.enabled);
        assert_eq!(binding.readiness, Readiness::Broken);
        assert!(binding.activity_bar.is_empty());
        assert_eq!(
            binding.reconciliation.as_ref().unwrap().observed,
            PluginObservedState::Broken
        );

        let mut optional_manifest = manifest;
        optional_manifest
            .ui
            .iter_mut()
            .find(|surface| surface.id == "standalone")
            .unwrap()
            .optional = true;
        let optional = installed_extension(optional_manifest, temp.path().to_path_buf(), true);
        let binding = project_extension_for_host(
            &optional,
            optional
                .surfaces()
                .into_iter()
                .map(str::to_string)
                .collect(),
            "0.3.0",
        )
        .await
        .unwrap();
        let reconciliation = binding.reconciliation.as_ref().unwrap();
        let standalone = reconciliation
            .surfaces
            .iter()
            .find(|surface| {
                surface.surface.kind == PluginSurfaceKind::Ui && surface.surface.id == "standalone"
            })
            .unwrap();
        assert!(binding.enabled);
        assert_eq!(binding.readiness, Readiness::Ready);
        assert_eq!(binding.activity_bar.len(), 1);
        assert_eq!(binding.activity_bar[0].id, "review");
        assert_eq!(reconciliation.observed, PluginObservedState::Degraded);
        assert_eq!(standalone.observed, SurfaceObservedState::Failed);
        assert!(!standalone.published);
    }

    #[cfg(feature = "extensions")]
    #[tokio::test]
    async fn schema_three_with_unobserved_runtime_surfaces_stays_unpublished() {
        let manifest = a3s_use_extension::ExtensionManifest::parse_acl(include_str!(
            "../crates/extension/fixtures/manifests/plugin-v3.acl"
        ))
        .unwrap();
        let temp = tempfile::tempdir().unwrap();
        let ui = temp.path().join("ui/review");
        tokio::fs::create_dir_all(&ui).await.unwrap();
        tokio::fs::write(ui.join("index.html"), b"<main>review</main>")
            .await
            .unwrap();
        tokio::fs::write(ui.join("index.css"), b"main { color: purple; }")
            .await
            .unwrap();
        tokio::fs::write(ui.join("index.js"), b"window.reviewReady = true;")
            .await
            .unwrap();
        for skill in ["review", "quick-look"] {
            let directory = temp.path().join("skills").join(skill);
            tokio::fs::create_dir_all(&directory).await.unwrap();
            tokio::fs::write(
                directory.join("SKILL.md"),
                format!("# {skill}\n\nVerified test skill.\n"),
            )
            .await
            .unwrap();
        }
        let extension = installed_extension(manifest, temp.path().to_path_buf(), true);
        let surfaces = extension
            .surfaces()
            .into_iter()
            .map(str::to_string)
            .collect();

        let binding = project_extension_for_host(&extension, surfaces, "0.3.0")
            .await
            .unwrap();
        let reconciliation = binding.reconciliation.as_ref().unwrap();

        assert!(!binding.enabled);
        assert_eq!(binding.readiness, Readiness::Unknown);
        assert!(binding.skills.is_empty());
        assert_eq!(reconciliation.observed, PluginObservedState::Reconciling);
        assert!(!reconciliation.capability_ready);
        assert!(reconciliation
            .surfaces
            .iter()
            .all(|surface| !surface.published));
    }

    #[tokio::test]
    async fn matching_revision_times_out_without_reporting_a_change() {
        let current = snapshot().await.unwrap();
        let changed = wait_for_change(
            current.generation,
            Some(&current.revision),
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        assert!(changed.is_none());
    }
}
