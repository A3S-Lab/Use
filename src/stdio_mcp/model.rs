use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use a3s_use_core::{
    PlanActor, PlanEnforcementProfile, PlanPolicyDecision, PluginSurfaceKind, PluginSurfaceRef,
    SurfacePermissionCeiling, UseError, UseResult, WorkspaceGrantAuthority,
};
use async_trait::async_trait;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};

use super::process_model::{StdioMcpProcessIdentity, StdioMcpProcessObservation};
use super::validation::{
    host_error, input_error, paths_overlap, valid_absolute_path,
    valid_authorization_recheck_interval, valid_machine_id, valid_package_id, valid_segment,
    valid_sha256, valid_timeout,
};

/// Process-local schema for an exact supervised stdio MCP session plan.
pub const STDIO_MCP_SESSION_PLAN_SCHEMA: &str = "a3s.use.stdio-mcp-session-plan.v1";

const DEFAULT_INITIALIZE_TIMEOUT_MS: u64 = 10_000;
const DEFAULT_SHUTDOWN_TIMEOUT_MS: u64 = 5_000;
const DEFAULT_AUTHORIZATION_RECHECK_INTERVAL_MS: u64 = 1_000;
const MAX_ARGUMENTS: usize = 256;
const MAX_ARGUMENT_BYTES: usize = 32 * 1024;

/// Host functionality independently required by the stdio compatibility
/// lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StdioMcpHostFeature {
    /// Start the package with no inherited package-visible environment.
    SanitizedEnvironment,
    /// Bind the exact package-data, temporary, and workspace roots.
    OwnedFilesystemRoots,
    /// Return a stable process identity tied to the session plan.
    ProcessIdentity,
    /// Drain stderr without treating it as MCP framing.
    StderrDrain,
    /// Stop the provider-owned process unit before reporting terminal state.
    ///
    /// This is lifecycle cleanup, not a claim that native-unconfined children
    /// cannot deliberately escape an OS process group.
    ProcessTreeCleanup,
    /// Enforce the reviewed filesystem allowlist.
    FilesystemConfinement,
    /// Enforce the reviewed network egress allowlist, including deny-all.
    NetworkEgressConfinement,
    /// Enforce whether descendants may be created.
    ChildProcessConfinement,
    /// Enforce CPU, memory, PID, and ephemeral-storage ceilings.
    ResourceConfinement,
}

/// Immutable capability evidence for one explicitly injected compatibility
/// host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpHostCapabilities {
    provider_id: String,
    provider_build_id: String,
    enforcement: PlanEnforcementProfile,
    features: Vec<StdioMcpHostFeature>,
    capability_digest: String,
}

impl StdioMcpHostCapabilities {
    /// Construct sorted canonical evidence and reject duplicate features.
    pub fn new(
        provider_id: impl Into<String>,
        provider_build_id: impl Into<String>,
        enforcement: PlanEnforcementProfile,
        mut features: Vec<StdioMcpHostFeature>,
    ) -> UseResult<Self> {
        features.sort();
        if features.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(host_error(
                "A stdio MCP host capability contains duplicate feature claims.",
            ));
        }
        let mut capabilities = Self {
            provider_id: provider_id.into(),
            provider_build_id: provider_build_id.into(),
            enforcement,
            features,
            capability_digest: String::new(),
        };
        capabilities.capability_digest = capabilities.calculate_digest()?;
        capabilities.validate()?;
        Ok(capabilities)
    }

    /// Stable provider identity selected by the trusted parent host.
    pub fn provider_id(&self) -> &str {
        &self.provider_id
    }

    /// Immutable provider implementation build.
    pub fn provider_build_id(&self) -> &str {
        &self.provider_build_id
    }

    /// Actual execution enforcement profile.
    pub fn enforcement(&self) -> PlanEnforcementProfile {
        self.enforcement
    }

    /// Canonically sorted enforcement and lifecycle features.
    pub fn features(&self) -> &[StdioMcpHostFeature] {
        &self.features
    }

    /// SHA-256 over the complete provider capability evidence.
    pub fn capability_digest(&self) -> &str {
        &self.capability_digest
    }

    pub(crate) fn validate_for(
        &self,
        permission: &SurfacePermissionCeiling,
        authority: &WorkspaceGrantAuthority,
    ) -> UseResult<()> {
        self.validate()?;
        for required in [
            StdioMcpHostFeature::SanitizedEnvironment,
            StdioMcpHostFeature::OwnedFilesystemRoots,
            StdioMcpHostFeature::ProcessIdentity,
            StdioMcpHostFeature::StderrDrain,
            StdioMcpHostFeature::ProcessTreeCleanup,
        ] {
            self.require_feature(required)?;
        }
        if !permission.native_execution || permission.private_service {
            return Err(host_error(
                "A stdio MCP compatibility session requires reviewed native, non-Service execution authority.",
            ));
        }
        if !permission.secrets.is_empty() {
            return Err(host_error(
                "Stdio MCP secret delivery is unavailable until the host supplies a typed secret-reference resolver.",
            ));
        }
        match self.enforcement {
            PlanEnforcementProfile::Sandbox => {
                for required in [
                    StdioMcpHostFeature::FilesystemConfinement,
                    StdioMcpHostFeature::NetworkEgressConfinement,
                    StdioMcpHostFeature::ChildProcessConfinement,
                    StdioMcpHostFeature::ResourceConfinement,
                ] {
                    self.require_feature(required)?;
                }
            }
            PlanEnforcementProfile::NativeUnconfined => {
                if authority.actor != PlanActor::User
                    || authority.decision != PlanPolicyDecision::Ask
                    || authority.confirmation_digest.is_none()
                {
                    return Err(UseError::new(
                        "use.plugin.stdio_mcp.native_confirmation_required",
                        "Native-unconfined stdio MCP requires an explicit user confirmation bound into the active workspace grant.",
                    ));
                }
            }
            PlanEnforcementProfile::Container => {
                return Err(host_error(
                    "A schema-v3 stdio MCP surface cannot be substituted with a container Service.",
                ));
            }
        }
        Ok(())
    }

    fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.provider_id)
            || !valid_machine_id(&self.provider_build_id)
            || self.features.windows(2).any(|pair| pair[0] >= pair[1])
            || !valid_sha256(&self.capability_digest)
            || self.calculate_digest()? != self.capability_digest
        {
            return Err(host_error(
                "A stdio MCP host capability has invalid identity, ordering, or digest evidence.",
            ));
        }
        Ok(())
    }

    fn require_feature(&self, feature: StdioMcpHostFeature) -> UseResult<()> {
        if self.features.binary_search(&feature).is_err() {
            return Err(UseError::new(
                "use.plugin.stdio_mcp.host_incapable",
                "The selected stdio MCP host cannot enforce the reviewed session contract.",
            )
            .with_detail("providerId", self.provider_id.clone())
            .with_detail(
                "missingFeature",
                serde_json::to_value(feature).unwrap_or(serde_json::Value::Null),
            ));
        }
        Ok(())
    }

    fn calculate_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestInput<'a> {
            provider_id: &'a str,
            provider_build_id: &'a str,
            enforcement: PlanEnforcementProfile,
            features: &'a [StdioMcpHostFeature],
        }

        digest_json(
            &DigestInput {
                provider_id: &self.provider_id,
                provider_build_id: &self.provider_build_id,
                enforcement: self.enforcement,
                features: &self.features,
            },
            "stdio MCP host capabilities",
        )
    }
}

/// Exact host filesystem roots made available to one stdio MCP session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpHostRoots {
    plugin_data_root: PathBuf,
    temporary_root: PathBuf,
    workspace_root: PathBuf,
}

impl StdioMcpHostRoots {
    /// Bind three disjoint absolute roots without ambient discovery.
    pub fn new(
        plugin_data_root: impl Into<PathBuf>,
        temporary_root: impl Into<PathBuf>,
        workspace_root: impl Into<PathBuf>,
    ) -> UseResult<Self> {
        let roots = Self {
            plugin_data_root: plugin_data_root.into(),
            temporary_root: temporary_root.into(),
            workspace_root: workspace_root.into(),
        };
        roots.validate()?;
        Ok(roots)
    }

    /// Package-owned durable data root.
    pub fn plugin_data_root(&self) -> &Path {
        &self.plugin_data_root
    }

    /// Session-owned temporary root.
    pub fn temporary_root(&self) -> &Path {
        &self.temporary_root
    }

    /// Explicit workspace root selected by the host.
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    fn validate(&self) -> UseResult<()> {
        let paths = [
            self.plugin_data_root.as_path(),
            self.temporary_root.as_path(),
            self.workspace_root.as_path(),
        ];
        if paths.iter().any(|path| !valid_absolute_path(path))
            || paths.iter().enumerate().any(|(index, left)| {
                paths[index + 1..]
                    .iter()
                    .any(|right| paths_overlap(left, right))
            })
        {
            return Err(input_error(
                "Stdio MCP package-data, temporary, and workspace roots must be disjoint normalized absolute paths.",
            ));
        }
        Ok(())
    }

    pub(crate) fn validate_against_package_root(&self, package_root: &Path) -> UseResult<()> {
        self.validate()?;
        if !valid_absolute_path(package_root)
            || [
                self.plugin_data_root.as_path(),
                self.temporary_root.as_path(),
                self.workspace_root.as_path(),
            ]
            .iter()
            .any(|root| paths_overlap(root, package_root))
        {
            return Err(input_error(
                "A stdio MCP package root must be absolute and disjoint from every writable host root.",
            ));
        }
        Ok(())
    }
}

/// Host-selected request for one exact package MCP session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StdioMcpSessionRequest {
    session_id: String,
    scope_id: String,
    package_id: String,
    surface_id: String,
    roots: StdioMcpHostRoots,
    initialize_timeout_ms: u64,
    shutdown_timeout_ms: u64,
    authorization_recheck_interval_ms: u64,
}

impl StdioMcpSessionRequest {
    /// Create a request with ten-second initialize, five-second shutdown, and
    /// one-second durable-authorization recheck bounds.
    pub fn new(
        session_id: impl Into<String>,
        scope_id: impl Into<String>,
        package_id: impl Into<String>,
        surface_id: impl Into<String>,
        roots: StdioMcpHostRoots,
    ) -> UseResult<Self> {
        let request = Self {
            session_id: session_id.into(),
            scope_id: scope_id.into(),
            package_id: package_id.into(),
            surface_id: surface_id.into(),
            roots,
            initialize_timeout_ms: DEFAULT_INITIALIZE_TIMEOUT_MS,
            shutdown_timeout_ms: DEFAULT_SHUTDOWN_TIMEOUT_MS,
            authorization_recheck_interval_ms: DEFAULT_AUTHORIZATION_RECHECK_INTERVAL_MS,
        };
        request.validate()?;
        Ok(request)
    }

    /// Override bounded initialization and shutdown deadlines.
    pub fn with_timeouts(
        mut self,
        initialize_timeout_ms: u64,
        shutdown_timeout_ms: u64,
    ) -> UseResult<Self> {
        self.initialize_timeout_ms = initialize_timeout_ms;
        self.shutdown_timeout_ms = shutdown_timeout_ms;
        self.validate()?;
        Ok(self)
    }

    /// Override the bounded durable-grant recheck interval.
    pub fn with_authorization_recheck_interval(
        mut self,
        authorization_recheck_interval_ms: u64,
    ) -> UseResult<Self> {
        self.authorization_recheck_interval_ms = authorization_recheck_interval_ms;
        self.validate()?;
        Ok(self)
    }

    /// Unique host-selected session identity.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Explicit workspace or user scope.
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Canonical `publisher/name` package.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Named schema-v3 MCP surface.
    pub fn surface_id(&self) -> &str {
        &self.surface_id
    }

    /// Exact host filesystem roots.
    pub fn roots(&self) -> &StdioMcpHostRoots {
        &self.roots
    }

    /// Initialization and provider-call deadline in milliseconds.
    pub fn initialize_timeout_ms(&self) -> u64 {
        self.initialize_timeout_ms
    }

    /// Graceful and forced shutdown deadline in milliseconds.
    pub fn shutdown_timeout_ms(&self) -> u64 {
        self.shutdown_timeout_ms
    }

    /// Maximum delay between durable workspace-grant rechecks.
    pub fn authorization_recheck_interval_ms(&self) -> u64 {
        self.authorization_recheck_interval_ms
    }

    fn validate(&self) -> UseResult<()> {
        self.roots.validate()?;
        if !valid_machine_id(&self.session_id)
            || !valid_machine_id(&self.scope_id)
            || !valid_package_id(&self.package_id)
            || !valid_segment(&self.surface_id)
            || !valid_timeout(self.initialize_timeout_ms)
            || !valid_timeout(self.shutdown_timeout_ms)
            || !valid_authorization_recheck_interval(self.authorization_recheck_interval_ms)
        {
            return Err(input_error(
                "A stdio MCP request has invalid session, scope, package, surface, or timeout fields.",
            ));
        }
        Ok(())
    }
}

/// Immutable package, authority, provider, and launch identity supplied to the
/// trusted process host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpSessionPlan {
    schema: String,
    session_id: String,
    scope_id: String,
    package_id: String,
    package_digest: String,
    receipt_digest: String,
    catalog_digest: String,
    manifest_digest: String,
    grant_revision: u64,
    grant_digest: String,
    grant_authority: WorkspaceGrantAuthority,
    grant_expires_at_ms: Option<u64>,
    permission_ceiling_digest: String,
    surface: PluginSurfaceRef,
    package_root: PathBuf,
    executable: PathBuf,
    args: Vec<String>,
    roots: StdioMcpHostRoots,
    permission: SurfacePermissionCeiling,
    non_secret_environment: BTreeMap<String, String>,
    provider: StdioMcpHostCapabilities,
    initialize_timeout_ms: u64,
    shutdown_timeout_ms: u64,
    authorization_recheck_interval_ms: u64,
    plan_digest: String,
}

pub(crate) struct StdioMcpPlanInput {
    pub request: StdioMcpSessionRequest,
    pub package_digest: String,
    pub receipt_digest: String,
    pub catalog_digest: String,
    pub manifest_digest: String,
    pub grant_revision: u64,
    pub grant_digest: String,
    pub grant_authority: WorkspaceGrantAuthority,
    pub grant_expires_at_ms: Option<u64>,
    pub permission_ceiling_digest: String,
    pub package_root: PathBuf,
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub permission: SurfacePermissionCeiling,
    pub provider: StdioMcpHostCapabilities,
}

impl StdioMcpSessionPlan {
    pub(crate) fn from_input(input: StdioMcpPlanInput) -> UseResult<Self> {
        let surface = PluginSurfaceRef {
            kind: PluginSurfaceKind::Mcp,
            id: input.request.surface_id.clone(),
        };
        let non_secret_environment =
            session_environment(&input.request, &input.package_root, &surface)?;
        let mut plan = Self {
            schema: STDIO_MCP_SESSION_PLAN_SCHEMA.to_string(),
            session_id: input.request.session_id,
            scope_id: input.request.scope_id,
            package_id: input.request.package_id,
            package_digest: input.package_digest,
            receipt_digest: input.receipt_digest,
            catalog_digest: input.catalog_digest,
            manifest_digest: input.manifest_digest,
            grant_revision: input.grant_revision,
            grant_digest: input.grant_digest,
            grant_authority: input.grant_authority,
            grant_expires_at_ms: input.grant_expires_at_ms,
            permission_ceiling_digest: input.permission_ceiling_digest,
            surface,
            package_root: input.package_root,
            executable: input.executable,
            args: input.args,
            roots: input.request.roots,
            permission: input.permission,
            non_secret_environment,
            provider: input.provider,
            initialize_timeout_ms: input.request.initialize_timeout_ms,
            shutdown_timeout_ms: input.request.shutdown_timeout_ms,
            authorization_recheck_interval_ms: input.request.authorization_recheck_interval_ms,
            plan_digest: String::new(),
        };
        plan.plan_digest = plan.calculate_digest()?;
        plan.validate()?;
        Ok(plan)
    }

    /// Process-local plan schema.
    pub fn schema(&self) -> &str {
        &self.schema
    }

    /// Unique session identity.
    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    /// Explicit lifecycle scope.
    pub fn scope_id(&self) -> &str {
        &self.scope_id
    }

    /// Canonical package identity.
    pub fn package_id(&self) -> &str {
        &self.package_id
    }

    /// Immutable expanded-package digest.
    pub fn package_digest(&self) -> &str {
        &self.package_digest
    }

    /// Named MCP surface.
    pub fn surface(&self) -> &PluginSurfaceRef {
        &self.surface
    }

    /// Immutable installed package root.
    pub fn package_root(&self) -> &Path {
        &self.package_root
    }

    /// Package-owned executable selected by the manifest.
    pub fn executable(&self) -> &Path {
        &self.executable
    }

    /// Exact manifest arguments.
    pub fn args(&self) -> &[String] {
        &self.args
    }

    /// Exact filesystem roots supplied by the host.
    pub fn roots(&self) -> &StdioMcpHostRoots {
        &self.roots
    }

    /// Resolved permission for this surface.
    pub fn permission(&self) -> &SurfacePermissionCeiling {
        &self.permission
    }

    /// Complete non-secret environment; providers must not add ambient variables.
    pub fn non_secret_environment(&self) -> &BTreeMap<String, String> {
        &self.non_secret_environment
    }

    /// Exact injected provider evidence.
    pub fn provider(&self) -> &StdioMcpHostCapabilities {
        &self.provider
    }

    /// Initialization deadline in milliseconds.
    pub fn initialize_timeout_ms(&self) -> u64 {
        self.initialize_timeout_ms
    }

    /// Graceful and forced shutdown deadline in milliseconds.
    pub fn shutdown_timeout_ms(&self) -> u64 {
        self.shutdown_timeout_ms
    }

    /// Maximum delay between durable workspace-grant rechecks.
    pub fn authorization_recheck_interval_ms(&self) -> u64 {
        self.authorization_recheck_interval_ms
    }

    /// SHA-256 over the complete session plan.
    pub fn plan_digest(&self) -> &str {
        &self.plan_digest
    }

    pub(crate) fn grant_revision(&self) -> u64 {
        self.grant_revision
    }

    pub(crate) fn grant_digest(&self) -> &str {
        &self.grant_digest
    }

    /// Exact actor, policy decision, and confirmation evidence.
    pub fn grant_authority(&self) -> &WorkspaceGrantAuthority {
        &self.grant_authority
    }

    /// Optional absolute host time after which the provider is terminated.
    pub fn grant_expires_at_ms(&self) -> Option<u64> {
        self.grant_expires_at_ms
    }

    pub(crate) fn receipt_digest(&self) -> &str {
        &self.receipt_digest
    }

    pub(crate) fn catalog_digest(&self) -> &str {
        &self.catalog_digest
    }

    pub(crate) fn manifest_digest(&self) -> &str {
        &self.manifest_digest
    }

    pub(crate) fn permission_ceiling_digest(&self) -> &str {
        &self.permission_ceiling_digest
    }

    fn validate(&self) -> UseResult<()> {
        self.provider.validate()?;
        self.roots
            .validate_against_package_root(&self.package_root)?;
        if self.schema != STDIO_MCP_SESSION_PLAN_SCHEMA
            || !valid_machine_id(&self.session_id)
            || !valid_machine_id(&self.scope_id)
            || !valid_package_id(&self.package_id)
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.receipt_digest)
            || !valid_sha256(&self.catalog_digest)
            || !valid_sha256(&self.manifest_digest)
            || self.grant_revision == 0
            || !valid_sha256(&self.grant_digest)
            || self.grant_authority.validate().is_err()
            || self.grant_expires_at_ms == Some(0)
            || !valid_sha256(&self.permission_ceiling_digest)
            || self.surface.kind != PluginSurfaceKind::Mcp
            || !valid_segment(&self.surface.id)
            || !valid_absolute_path(&self.executable)
            || self.executable == self.package_root
            || !self.executable.starts_with(&self.package_root)
            || self.args.len() > MAX_ARGUMENTS
            || self.args.iter().any(|value| {
                value.is_empty() || value.len() > MAX_ARGUMENT_BYTES || value.contains('\0')
            })
            || self.permission.surface != self.surface
            || !self.permission.native_execution
            || self.permission.private_service
            || !self.permission.secrets.is_empty()
            || !valid_timeout(self.initialize_timeout_ms)
            || !valid_timeout(self.shutdown_timeout_ms)
            || !valid_authorization_recheck_interval(self.authorization_recheck_interval_ms)
            || !valid_sha256(&self.plan_digest)
            || self.calculate_digest()? != self.plan_digest
        {
            return Err(input_error(
                "A stdio MCP session plan contains invalid or inconsistent immutable evidence.",
            ));
        }
        Ok(())
    }

    fn calculate_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct DigestInput<'a> {
            schema: &'a str,
            session_id: &'a str,
            scope_id: &'a str,
            package_id: &'a str,
            package_digest: &'a str,
            receipt_digest: &'a str,
            catalog_digest: &'a str,
            manifest_digest: &'a str,
            grant_revision: u64,
            grant_digest: &'a str,
            grant_authority: &'a WorkspaceGrantAuthority,
            grant_expires_at_ms: Option<u64>,
            permission_ceiling_digest: &'a str,
            surface: &'a PluginSurfaceRef,
            package_root: &'a Path,
            executable: &'a Path,
            args: &'a [String],
            roots: &'a StdioMcpHostRoots,
            permission: &'a SurfacePermissionCeiling,
            non_secret_environment: &'a BTreeMap<String, String>,
            provider: &'a StdioMcpHostCapabilities,
            initialize_timeout_ms: u64,
            shutdown_timeout_ms: u64,
            authorization_recheck_interval_ms: u64,
        }

        digest_json(
            &DigestInput {
                schema: &self.schema,
                session_id: &self.session_id,
                scope_id: &self.scope_id,
                package_id: &self.package_id,
                package_digest: &self.package_digest,
                receipt_digest: &self.receipt_digest,
                catalog_digest: &self.catalog_digest,
                manifest_digest: &self.manifest_digest,
                grant_revision: self.grant_revision,
                grant_digest: &self.grant_digest,
                grant_authority: &self.grant_authority,
                grant_expires_at_ms: self.grant_expires_at_ms,
                permission_ceiling_digest: &self.permission_ceiling_digest,
                surface: &self.surface,
                package_root: &self.package_root,
                executable: &self.executable,
                args: &self.args,
                roots: &self.roots,
                permission: &self.permission,
                non_secret_environment: &self.non_secret_environment,
                provider: &self.provider,
                initialize_timeout_ms: self.initialize_timeout_ms,
                shutdown_timeout_ms: self.shutdown_timeout_ms,
                authorization_recheck_interval_ms: self.authorization_recheck_interval_ms,
            },
            "stdio MCP session plan",
        )
    }
}

/// Trusted process control retained by the Use-owned session state machine.
#[async_trait]
pub trait StdioMcpProcessControl: Send + Sync {
    /// Exact immutable identity returned by spawn.
    fn identity(&self) -> &StdioMcpProcessIdentity;

    /// Observe the exact provider-owned process-unit state.
    async fn observe(&self) -> UseResult<StdioMcpProcessObservation>;

    /// Wait until the complete provider-owned process unit is terminal.
    ///
    /// An error is not terminal evidence. The supervisor retains the package
    /// lease, requests termination, and may call this method again until an
    /// exact terminal observation is returned.
    async fn wait_for_exit(&self) -> UseResult<StdioMcpProcessObservation>;

    /// Initiate nonblocking idempotent process-unit termination.
    fn terminate(&self);
}

/// Provider-owned stdio pipes plus exact process control.
pub struct SpawnedStdioMcpSession {
    reader: Box<dyn AsyncRead + Send + Unpin>,
    writer: Box<dyn AsyncWrite + Send + Unpin>,
    control: Arc<dyn StdioMcpProcessControl>,
}

impl SpawnedStdioMcpSession {
    /// Package typed stdio pipes and process control returned by a provider.
    pub fn new<R, W>(
        reader: R,
        writer: W,
        control: Arc<dyn StdioMcpProcessControl>,
    ) -> UseResult<Self>
    where
        R: AsyncRead + Send + Unpin + 'static,
        W: AsyncWrite + Send + Unpin + 'static,
    {
        control.identity().validate()?;
        Ok(Self {
            reader: Box::new(reader),
            writer: Box::new(writer),
            control,
        })
    }

    pub(crate) fn into_parts(
        self,
    ) -> (
        Box<dyn AsyncRead + Send + Unpin>,
        Box<dyn AsyncWrite + Send + Unpin>,
        Arc<dyn StdioMcpProcessControl>,
    ) {
        (self.reader, self.writer, self.control)
    }
}

impl fmt::Debug for SpawnedStdioMcpSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("SpawnedStdioMcpSession")
            .field("identity", self.control.identity())
            .field("reader", &"[STDIO]")
            .field("writer", &"[STDIO]")
            .finish()
    }
}

/// Explicitly injected compatibility host. Packages cannot register or select
/// this provider.
#[async_trait]
pub trait StdioMcpHostProvider: Send + Sync {
    /// Return current immutable provider capability evidence.
    async fn capabilities(&self) -> UseResult<StdioMcpHostCapabilities>;

    /// Spawn exactly; error or future cancellation must leave no process alive.
    async fn spawn(&self, plan: &StdioMcpSessionPlan) -> UseResult<SpawnedStdioMcpSession>;
}

fn session_environment(
    request: &StdioMcpSessionRequest,
    package_root: &Path,
    surface: &PluginSurfaceRef,
) -> UseResult<BTreeMap<String, String>> {
    let path_value = |path: &Path| {
        path.to_str().map(str::to_owned).ok_or_else(|| {
            input_error("Stdio MCP host roots must be representable in the package environment.")
        })
    };
    Ok(BTreeMap::from([
        (
            "A3S_USE_EXTENSION_ID".to_string(),
            request.package_id.clone(),
        ),
        ("A3S_USE_MCP_SURFACE_ID".to_string(), surface.id.clone()),
        (
            "A3S_USE_PACKAGE_ROOT".to_string(),
            path_value(package_root)?,
        ),
        (
            "A3S_USE_PLUGIN_DATA_ROOT".to_string(),
            path_value(request.roots.plugin_data_root())?,
        ),
        ("A3S_USE_SCOPE_ID".to_string(), request.scope_id.clone()),
        ("A3S_USE_SESSION_ID".to_string(), request.session_id.clone()),
        (
            "A3S_USE_TEMP_ROOT".to_string(),
            path_value(request.roots.temporary_root())?,
        ),
        (
            "A3S_USE_WORKSPACE_ROOT".to_string(),
            path_value(request.roots.workspace_root())?,
        ),
    ]))
}

fn digest_json(value: &impl Serialize, label: &str) -> UseResult<String> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        input_error(format!(
            "Failed to encode canonical {label} evidence: {error}"
        ))
    })?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}
