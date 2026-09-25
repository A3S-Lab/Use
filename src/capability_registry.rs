//! Unified projection of host-seeded and externally installed Use capabilities.
//!
//! This is a versioned JSON CLI contract for long-running consumers. It is
//! not a private RPC protocol: invocation still happens through native CLI,
//! standard MCP, and `SKILL.md` surfaces.
//!
//! First-party domains (Browser, OCR, Box, …) are never hardcoded into the
//! universal engine path. Product hosts inject them through
//! [`CapabilitySeedProvider`]; bare [`CapabilityRegistry::new`] projects none.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use a3s_use_core::{
    CapabilityDescriptionProof, CapabilityDescriptor, CapabilityGatewayCatalog, InstallationId,
    InstalledPluginPlanEvidence, OkfCapabilityProjection, PlanScope, PluginSurfaceRef, Readiness,
    UseError, UseResult,
};
#[cfg(feature = "extensions")]
use a3s_use_core::{
    InstallationSnapshot, PlanQualifiedSurfaceRef, PluginSurfaceKind,
    INSTALLED_PLUGIN_PLAN_EVIDENCE_SCHEMA,
};
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;

#[cfg(feature = "extensions")]
#[path = "capability_registry/runtime_tasks.rs"]
mod runtime_tasks;
#[cfg(feature = "extensions")]
use runtime_tasks::runtime_task_evidence_from_store;
#[cfg(feature = "extensions")]
#[path = "capability_registry/mcp.rs"]
mod managed_mcp;
#[cfg(feature = "extensions")]
use managed_mcp::mcp_evidence_from_store;
#[cfg(feature = "extensions")]
#[path = "capability_registry/executable_tools.rs"]
mod executable_tools;
#[cfg(feature = "extensions")]
use executable_tools::executable_tool_evidence_from_package;
#[path = "capability_registry/lease.rs"]
mod lease;
use lease::CapabilityUpstreamEvidence;
pub use lease::{
    acquire_snapshot_lease, CapabilityPackageGeneration, CapabilitySnapshotCursor,
    CapabilitySnapshotLease, CAPABILITY_SNAPSHOT_CURSOR_SCHEMA,
};
#[path = "capability_registry/product_seeds.rs"]
mod product_seeds;
pub use product_seeds::BundledFirstPartyCapabilitySeeds;

#[cfg(feature = "extensions")]
pub use crate::surface_reconciler::{
    ReconciledSurface, SurfaceDesiredState, SurfaceObservedState, SurfaceOwner,
    SurfaceReconcileSnapshot, SurfaceStateReason,
};

#[cfg(feature = "extensions")]
use crate::surface_reconciler::{
    reconcile_with_runtime_and_knowledge, PluginDesiredState, PluginObservedState,
    SurfaceObservations,
};
#[cfg(feature = "extensions")]
use crate::{
    flow_runtime::FlowRuntimeBindingStore,
    okf_knowledge::{
        OkfKnowledgeBinding, OkfKnowledgeBindingStore, OkfKnowledgeClient,
        SqliteOkfKnowledgeAdapter,
    },
    plugin_runtime::RuntimeBindingStore,
};

pub const CAPABILITY_REGISTRY_SCHEMA_VERSION: u32 = 5;
pub const UI_DEPENDENCY_EVIDENCE_SCHEMA: &str = "a3s.use.ui-dependency-evidence.v1";
#[cfg(feature = "extensions")]
pub(crate) const PLANNER_EVIDENCE_SCHEMA_VERSION: u32 = 1;
const MAX_FLOW_SOURCE_BYTES: u64 = 4 * 1024 * 1024;
#[cfg(feature = "extensions")]
pub(crate) const MAX_STABLE_SNAPSHOT_ATTEMPTS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityRegistrySnapshot {
    pub schema_version: u32,
    pub installation: InstallationId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_snapshot_digest: Option<String>,
    pub generation: u64,
    pub revision: String,
    pub capabilities: Vec<CapabilityBinding>,
    #[serde(skip)]
    cursor: CapabilitySnapshotCursor,
}

/// Host-composed capability projection over one authoritative Use Registry.
///
/// Managed hosts inject the same [`a3s_use_extension::ExtensionRegistry`]
/// used by package planning and lifecycle cutover. [`Self::from_env`] is only
/// the standalone facade composition.
#[derive(Debug, Clone)]
pub struct CapabilityRegistry {
    #[cfg(feature = "extensions")]
    extensions: a3s_use_extension::ExtensionRegistry,
    #[cfg(not(feature = "extensions"))]
    installation: InstallationId,
    /// Host-injected first-party seeds. Empty for the universal engine.
    seeds: Arc<dyn CapabilitySeedProvider>,
}

/// Host-supplied capability seeds projected ahead of installed extensions.
///
/// Product profiles inject Browser/OCR/Box (and similar) here. The universal
/// engine path uses [`EmptyCapabilitySeeds`] and never names those domains.
#[async_trait]
pub trait CapabilitySeedProvider: Send + Sync + std::fmt::Debug {
    async fn project(&self) -> UseResult<Vec<CapabilityBinding>>;
}

/// Universal-engine seed set: no first-party domains.
#[derive(Debug, Default, Clone, Copy)]
pub struct EmptyCapabilitySeeds;

#[async_trait]
impl CapabilitySeedProvider for EmptyCapabilitySeeds {
    async fn project(&self) -> UseResult<Vec<CapabilityBinding>> {
        Ok(Vec::new())
    }
}

impl CapabilityRegistry {
    /// Product standalone composition: env-backed extensions plus bundled
    /// first-party seeds (Browser/OCR/Box). Prefer [`Self::new`] or
    /// [`Self::with_seeds`] when embedding a domain-neutral engine.
    pub fn from_env(installation: InstallationId) -> UseResult<Self> {
        Self::from_env_with_seeds(installation, Arc::new(BundledFirstPartyCapabilitySeeds))
    }

    /// Env-backed registry with an explicit seed provider.
    pub fn from_env_with_seeds(
        installation: InstallationId,
        seeds: Arc<dyn CapabilitySeedProvider>,
    ) -> UseResult<Self> {
        installation.validate()?;
        Ok(Self {
            #[cfg(feature = "extensions")]
            extensions: a3s_use_extension::ExtensionRegistry::from_env(installation)?,
            #[cfg(not(feature = "extensions"))]
            installation,
            seeds,
        })
    }

    /// Universal engine constructor: installed extensions only, no hardcoded
    /// first-party domains.
    #[cfg(feature = "extensions")]
    pub fn new(extensions: a3s_use_extension::ExtensionRegistry) -> Self {
        Self::with_seeds(extensions, Arc::new(EmptyCapabilitySeeds))
    }

    #[cfg(not(feature = "extensions"))]
    pub fn new(installation: InstallationId) -> UseResult<Self> {
        Self::with_seeds(installation, Arc::new(EmptyCapabilitySeeds))
    }

    /// Compose a registry with explicit host-owned capability seeds.
    #[cfg(feature = "extensions")]
    pub fn with_seeds(
        extensions: a3s_use_extension::ExtensionRegistry,
        seeds: Arc<dyn CapabilitySeedProvider>,
    ) -> Self {
        Self { extensions, seeds }
    }

    #[cfg(not(feature = "extensions"))]
    pub fn with_seeds(
        installation: InstallationId,
        seeds: Arc<dyn CapabilitySeedProvider>,
    ) -> UseResult<Self> {
        installation.validate()?;
        Ok(Self {
            installation,
            seeds,
        })
    }

    pub fn installation(&self) -> &InstallationId {
        #[cfg(feature = "extensions")]
        {
            self.extensions.installation()
        }
        #[cfg(not(feature = "extensions"))]
        {
            &self.installation
        }
    }

    #[cfg(feature = "extensions")]
    pub fn extension_registry(&self) -> &a3s_use_extension::ExtensionRegistry {
        &self.extensions
    }

    pub async fn snapshot(&self) -> UseResult<CapabilityRegistrySnapshot> {
        #[cfg(feature = "extensions")]
        let (extension_cursor, extensions) = stable_extensions(&self.extensions).await?;
        #[cfg(not(feature = "extensions"))]
        let (extension_cursor, extensions) = stable_extensions(self.installation()).await?;
        let mut capabilities = self.seeds.project().await?;
        capabilities.extend(extensions);
        capabilities.sort_by(|left, right| left.id.cmp(&right.id));
        validate_unique_tool_task_names(&capabilities)?;
        validate_unique_executable_tool_names(&capabilities)?;
        validate_unique_mcp_server_names(&capabilities)?;

        let revision = revision(
            self.installation(),
            extension_cursor.installation_generation(),
            extension_cursor.installation_snapshot_digest(),
            &capabilities,
        )?;
        let cursor = CapabilitySnapshotCursor::from_projection(&revision, extension_cursor)?;
        Ok(CapabilityRegistrySnapshot {
            schema_version: CAPABILITY_REGISTRY_SCHEMA_VERSION,
            installation: self.installation().clone(),
            installation_generation: cursor.installation_generation,
            installation_snapshot_digest: cursor.installation_snapshot_digest.clone(),
            generation: cursor.generation,
            revision,
            capabilities,
            cursor,
        })
    }

    pub async fn acquire_snapshot_lease(
        &self,
        expected: &CapabilitySnapshotCursor,
    ) -> UseResult<Option<CapabilitySnapshotLease>> {
        lease::acquire_snapshot_lease_from(self, expected).await
    }

    pub async fn wait_for_change(
        &self,
        after_generation: u64,
        after_revision: Option<&str>,
        timeout: Duration,
    ) -> UseResult<Option<CapabilityRegistrySnapshot>> {
        let current = self.snapshot().await?;
        if snapshot_changed(&current, after_generation, after_revision) {
            return Ok(Some(current));
        }
        let deadline = Instant::now().checked_add(timeout).ok_or_else(|| {
            UseError::new(
                "use.capability.timeout_invalid",
                "The capability watch timeout is too large.",
            )
        })?;

        #[cfg(feature = "extensions")]
        {
            if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                if self
                    .extensions
                    .wait_for_change(after_generation, remaining)
                    .await?
                    .is_some()
                {
                    let current = self.snapshot().await?;
                    if snapshot_changed(&current, after_generation, after_revision) {
                        return Ok(Some(current));
                    }
                    return Err(UseError::new(
                        "use.capability.notification_invalid",
                        "The extension Registry changed without advancing the capability publication.",
                    ));
                }
            }
        }

        #[cfg(not(feature = "extensions"))]
        if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            tokio::time::sleep(remaining).await;
        }

        // A final projection closes the notification-to-timeout race and
        // preserves revision-based detection for custom builds without the
        // extension lifecycle publisher. The normal wait path performs no
        // fixed-interval projection, filesystem scan, or asset hashing.
        let current = self.snapshot().await?;
        Ok(snapshot_changed(&current, after_generation, after_revision).then_some(current))
    }
}

fn snapshot_changed(
    current: &CapabilityRegistrySnapshot,
    after_generation: u64,
    after_revision: Option<&str>,
) -> bool {
    match after_revision {
        Some(revision) => current.generation != after_generation || current.revision != revision,
        None => current.generation > after_generation,
    }
}

impl CapabilityRegistrySnapshot {
    pub fn cursor(&self) -> &CapabilitySnapshotCursor {
        &self.cursor
    }

    /// Materialize the agent-facing Capability Gateway catalog from this
    /// already stable Use publication.
    ///
    /// Descriptors are supplied by the managed host because only that host
    /// can verify a package's signed description and bind it to a concrete
    /// provider. This method performs the common publication checks before a
    /// catalog can be handed to the Gateway: every descriptor must belong to
    /// an enabled, ready, plan-qualified package in this exact snapshot,
    /// carry the package lifecycle identity from the snapshot cursor, and
    /// reference the same verified catalog record as the package evidence.
    /// A subset of the published surfaces is allowed; hosts may intentionally
    /// expose only the capabilities authorized for a particular consumer.
    pub fn capability_gateway_catalog(
        &self,
        descriptors: Vec<CapabilityDescriptor>,
    ) -> UseResult<CapabilityGatewayCatalog> {
        self.cursor.validate()?;
        if self.schema_version != CAPABILITY_REGISTRY_SCHEMA_VERSION
            || self.cursor.installation != self.installation
            || self.cursor.installation_generation != self.installation_generation
            || self.cursor.installation_snapshot_digest != self.installation_snapshot_digest
            || self.cursor.generation != self.generation
            || self.cursor.revision != self.revision
        {
            return Err(gateway_catalog_projection_error(
                "The capability snapshot cursor does not match its public snapshot identity.",
            ));
        }
        let expected_revision = revision(
            &self.installation,
            self.installation_generation,
            self.installation_snapshot_digest.as_deref(),
            &self.capabilities,
        )?;
        if expected_revision != self.revision {
            return Err(gateway_catalog_projection_error(
                "The capability snapshot projection does not match its immutable revision.",
            ));
        }

        for descriptor in &descriptors {
            descriptor.validate()?;
            let package_id = descriptor.package_id.to_string();
            let package = self
                .cursor
                .packages
                .iter()
                .find(|package| package.package_id == package_id)
                .ok_or_else(|| {
                    gateway_catalog_projection_error(
                        "A Gateway descriptor belongs to a package outside the exact Use snapshot.",
                    )
                })?;
            if package.lifecycle_generation != descriptor.generation
                || package.package_digest != descriptor.package_digest
                || package.manifest_digest != descriptor.manifest_digest
            {
                return Err(gateway_catalog_projection_error(
                    "A Gateway descriptor does not match its package lifecycle identity.",
                ));
            }

            let evidence = self
                .capabilities
                .iter()
                .filter_map(|binding| binding.planner_evidence.as_ref())
                .find(|evidence| evidence.package_id == package_id)
                .ok_or_else(|| {
                    gateway_catalog_projection_error(
                        "A Gateway descriptor lacks reviewed package publication evidence.",
                    )
                })?;
            if evidence.package_sha256 != descriptor.package_digest
                || evidence.manifest_sha256 != descriptor.manifest_digest
                || !evidence.desired_enabled
                || evidence.catalog_record_digest != descriptor.publication.catalog_record_digest
                || !evidence.selected_surfaces.contains(&descriptor.surface)
            {
                return Err(gateway_catalog_projection_error(
                    "A Gateway descriptor is not bound to the reviewed, enabled package surface.",
                ));
            }

            let binding = self
                .capabilities
                .iter()
                .find(|binding| {
                    binding
                        .planner_evidence
                        .as_ref()
                        .is_some_and(|candidate| candidate.package_id == package_id)
                })
                .ok_or_else(|| {
                    gateway_catalog_projection_error(
                        "A Gateway descriptor package projection is missing.",
                    )
                })?;
            if !binding.enabled || binding.readiness != Readiness::Ready {
                return Err(gateway_catalog_projection_error(
                    "A Gateway descriptor package is not ready for publication.",
                ));
            }
        }

        CapabilityGatewayCatalog::new(self.installation.clone(), self.generation, descriptors)
    }

    /// Materialize a Gateway catalog from host-verified descriptions.  The
    /// proof envelope is consumed before the ordinary snapshot projection so
    /// a caller cannot accidentally bypass the signed-description hand-off
    /// when composing a production Gateway.
    pub fn capability_gateway_catalog_from_verified_descriptions(
        &self,
        proofs: Vec<CapabilityDescriptionProof>,
    ) -> UseResult<CapabilityGatewayCatalog> {
        let descriptors = proofs
            .into_iter()
            .map(|proof| {
                proof.validate()?;
                Ok(proof.into_descriptor())
            })
            .collect::<UseResult<Vec<_>>>()?;
        self.capability_gateway_catalog(descriptors)
    }

    /// Materialize a Gateway catalog from signed descriptions after the
    /// extension trust boundary has verified every envelope. This adapter is
    /// the preferred production hand-off: callers supply public-key policy,
    /// never a signer string or a forged core proof. The trust-store source is
    /// still host-owned and must come from the Registry/TUF authority.
    #[cfg(feature = "extensions")]
    pub fn capability_gateway_catalog_from_signed_descriptions(
        &self,
        signed: Vec<a3s_use_core::SignedCapabilityDescription>,
        trust_store: &a3s_use_extension::CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
    ) -> UseResult<CapabilityGatewayCatalog> {
        let proofs = signed
            .into_iter()
            .map(|envelope| {
                trust_store
                    .verify(&envelope, now_unix_seconds)?
                    .into_proof()
            })
            .collect::<UseResult<Vec<_>>>()?;
        self.capability_gateway_catalog_from_verified_descriptions(proofs)
    }

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

fn gateway_catalog_projection_error(message: impl Into<String>) -> UseError {
    UseError::new("use.capability.gateway_catalog_projection_invalid", message)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityOrigin {
    BuiltIn,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpTransport {
    Stdio,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpSurface {
    pub target: String,
    pub transport: McpTransport,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpSurfaceActivation {
    Eager,
    Lazy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpRuntimeProjection {
    pub scope: PlanScope,
    pub endpoint_ref: String,
    pub endpoint_path: String,
    pub protocol_version: String,
    pub initialized_at_ms: u64,
    pub provider_id: String,
    pub provider_build_id: String,
    pub runtime_generation: u64,
    pub descriptor_digest: String,
    pub binding_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(
    tag = "transport",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum McpLaunchProjection {
    Stdio {
        executable: PathBuf,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        args: Vec<String>,
    },
    StreamableHttp {
        release: PathBuf,
        runtime: McpRuntimeProjection,
    },
    /// Host-configured endpoint. The projection carries the signed allowlist
    /// only; the URL and authorization stay in the host grant store.
    HostGrant {
        contract: PathBuf,
        contract_digest: String,
        allowed_hosts: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct McpServerProjection {
    pub id: String,
    pub server_name: String,
    pub activation: McpSurfaceActivation,
    pub lifecycle_identity: ProjectedLifecycleIdentity,
    pub file_evidence_digest: String,
    pub launch: McpLaunchProjection,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SkillSurface {
    pub id: String,
    pub path: PathBuf,
    pub sha256: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowEngine {
    A3sFlow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FlowRuntime {
    NativeTs,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FlowSurface {
    pub id: String,
    pub engine: FlowEngine,
    pub runtime: FlowRuntime,
    pub source: ManagedAsset,
    pub export_name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_tools: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_mcp: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub requires_okf: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectedLifecycleIdentity {
    pub package_id: String,
    pub package_digest: String,
    pub manifest_digest: String,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolTaskProjection {
    pub tool_name: String,
    pub surface_id: String,
    pub command: String,
    pub json_output: bool,
    pub timeout_ms: u64,
    pub scope: PlanScope,
    pub lifecycle_identity: ProjectedLifecycleIdentity,
    pub provider_id: String,
}

/// Package-local Executable Tool projection (integrity-bound files, not Runtime
/// BindingStore receipts). Hosts reinspect file evidence and spawn under the
/// package root — never invent a provider_id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutableToolProjection {
    pub tool_name: String,
    pub surface_id: String,
    pub command: String,
    pub json_output: bool,
    pub timeout_ms: u64,
    pub scope: PlanScope,
    pub lifecycle_identity: ProjectedLifecycleIdentity,
    pub file_evidence_digest: String,
    pub executable: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedAsset {
    pub path: PathBuf,
    pub sha256: String,
    pub media_type: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityBarContribution {
    pub id: String,
    pub title: String,
    pub description: String,
    pub icon: String,
    pub entry: ManagedAsset,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub styles: Vec<ManagedAsset>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<ManagedAsset>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
    pub dependency_evidence_schema: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginSurfaceRef>,
    pub order: i32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepositoryBinding {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PluginPlannerEvidence {
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
pub struct CapabilityBinding {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    pub version: String,
    pub origin: CapabilityOrigin,
    pub enabled: bool,
    pub readiness: Readiness,
    #[cfg(feature = "extensions")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reconciliation: Option<SurfaceReconcileSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub planner_evidence: Option<PluginPlannerEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub package_root: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lifecycle_generation: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requires_use: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repository: Option<RepositoryBinding>,
    pub surfaces: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<McpSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mcp_servers: Vec<McpServerProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub skills: Vec<SkillSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flows: Vec<FlowSurface>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub knowledge: Vec<OkfCapabilityProjection>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub activity_bar: Vec<ActivityBarContribution>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_tasks: Vec<ToolTaskProjection>,
    /// Package-local Executable Tools (host-projected; often empty until wired).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub executable_tools: Vec<ExecutableToolProjection>,
}

pub async fn snapshot(installation: InstallationId) -> UseResult<CapabilityRegistrySnapshot> {
    CapabilityRegistry::from_env(installation)?.snapshot().await
}

fn validate_unique_executable_tool_names(capabilities: &[CapabilityBinding]) -> UseResult<()> {
    let mut names = std::collections::BTreeSet::new();
    for tool in capabilities
        .iter()
        .flat_map(|capability| capability.executable_tools.iter())
    {
        if !names.insert(tool.tool_name.as_str()) {
            return Err(UseError::new(
                "use.capability.executable_tool_name_conflict",
                "Two package-local Executable Tools resolve to the same host tool identity.",
            ));
        }
    }
    for capability in capabilities {
        for tool in &capability.executable_tools {
            if capability
                .tool_tasks
                .iter()
                .any(|task| task.tool_name == tool.tool_name)
            {
                return Err(UseError::new(
                    "use.capability.tool_name_conflict",
                    "A package-local Executable Tool and a Runtime Tool Task share the same host tool identity.",
                ));
            }
        }
    }
    Ok(())
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

fn validate_unique_mcp_server_names(capabilities: &[CapabilityBinding]) -> UseResult<()> {
    let mut names = std::collections::BTreeSet::new();
    for server in capabilities
        .iter()
        .flat_map(|capability| capability.mcp_servers.iter())
    {
        if !names.insert(server.server_name.as_str()) {
            return Err(UseError::new(
                "use.capability.mcp_name_conflict",
                "Two MCP surfaces resolve to the same host server identity.",
            ));
        }
    }
    Ok(())
}

#[cfg(feature = "extensions")]
pub(crate) async fn installed_plugin_plan_evidence(
    installation: InstallationId,
    package_id: &str,
) -> UseResult<InstalledPluginPlanEvidence> {
    let snapshot = snapshot(installation.clone()).await?;
    let extension = crate::extension_host::get(installation, package_id)
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
    _installation: InstallationId,
    _package_id: &str,
) -> UseResult<InstalledPluginPlanEvidence> {
    Err(UseError::new(
        "use.extension.disabled",
        "External extension support is disabled in this custom build.",
    ))
}

pub async fn wait_for_change(
    installation: InstallationId,
    after_generation: u64,
    after_revision: Option<&str>,
    timeout: Duration,
) -> UseResult<Option<CapabilityRegistrySnapshot>> {
    CapabilityRegistry::from_env(installation)?
        .wait_for_change(after_generation, after_revision, timeout)
        .await
}

fn revision(
    installation: &InstallationId,
    installation_generation: Option<u64>,
    installation_snapshot_digest: Option<&str>,
    capabilities: &[CapabilityBinding],
) -> UseResult<String> {
    let bytes = serde_json::to_vec(&(
        "a3s.use.capability-revision.v4",
        installation,
        installation_generation,
        installation_snapshot_digest,
        capabilities,
    ))
    .map_err(|error| {
        UseError::new(
            "use.capability.snapshot_invalid",
            format!("Failed to encode the capability snapshot: {error}"),
        )
    })?;
    let digest = Sha256::digest(bytes);
    Ok(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

pub(super) async fn skill_surface(id: &str, path: PathBuf) -> UseResult<SkillSurface> {
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
        id: id.to_owned(),
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

#[path = "capability_registry/extension_projection.rs"]
mod extension_projection;
#[cfg(not(feature = "extensions"))]
use extension_projection::stable_extensions;
#[cfg(feature = "extensions")]
pub(crate) use extension_projection::{
    installed_plugin_plan_evidence_from_snapshot, knowledge_evidence_from_store,
    project_extension_for_host_with_evidence, stable_extensions, CapabilityHostProjectionContext,
    KnowledgeEvidence,
};
#[cfg(all(feature = "extensions", test))]
pub(crate) use extension_projection::project_extension_for_host;

#[cfg(all(test, feature = "extensions"))]
#[path = "capability_registry_planner_tests.rs"]
mod planner_tests;

#[cfg(test)]
#[path = "capability_registry/registry_tests.rs"]
mod tests;
