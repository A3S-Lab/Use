use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{PluginSurfaceKind, SurfacePermissionCeiling, UseError, UseResult};
use a3s_use_extension::{
    InstalledExtension, PluginMcpLaunch, StoredWorkspaceGrant, WorkspaceGrantReceipt,
    WorkspaceGrantStore,
};
use rmcp::model::{ClientCapabilities, ClientInfo, Implementation, ProtocolVersion, ServerInfo};
use rmcp::service::{QuitReason, RunningService};
use rmcp::{Peer, RoleClient, ServiceExt};
use serde::Serialize;

use crate::{
    CapabilityHostSurfaceObservation, CapabilityHostSurfaceOwner, CapabilitySurfaceObservedState,
};

use super::authorization::{AuthorizationMonitor, StdioMcpAuthorizationObservation};
use super::model::{
    StdioMcpHostProvider, StdioMcpPlanInput, StdioMcpProcessControl, StdioMcpSessionPlan,
    StdioMcpSessionRequest,
};
use super::process_model::{
    StdioMcpProcessIdentity, StdioMcpProcessObservation, StdioMcpProcessState,
};
use super::settlement::{LeaseSettlement, StdioMcpPackageLease};
use super::transport::BoundedStdioTransport;
use super::validation::{unix_time_ms, valid_protocol, valid_server_text};

type RunningStdioClient = RunningService<RoleClient, ClientInfo>;

/// Validated but not yet spawned compatibility session.
#[derive(Debug)]
pub struct PreparedStdioMcpSession {
    plan: StdioMcpSessionPlan,
}

impl PreparedStdioMcpSession {
    /// Complete immutable launch and authority plan.
    pub fn plan(&self) -> &StdioMcpSessionPlan {
        &self.plan
    }

    /// Scope/package-bound observation used to publish a lazy stdio MCP
    /// surface only while the caller retains the matching package lease.
    pub fn host_observation(&self) -> UseResult<CapabilityHostSurfaceObservation> {
        capability_observation(&self.plan, CapabilitySurfaceObservedState::Prepared)
    }
}

/// Standard MCP initialize evidence retained for a live stdio session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpInitializeEvidence {
    protocol_version: String,
    server_name: String,
    server_version: String,
    initialized_at_ms: u64,
}

impl StdioMcpInitializeEvidence {
    /// Negotiated standard MCP protocol version.
    pub fn protocol_version(&self) -> &str {
        &self.protocol_version
    }

    /// Server implementation name returned by MCP initialize.
    pub fn server_name(&self) -> &str {
        &self.server_name
    }

    /// Server implementation version returned by MCP initialize.
    pub fn server_version(&self) -> &str {
        &self.server_version
    }

    /// Host time after a successful initialize exchange.
    pub fn initialized_at_ms(&self) -> u64 {
        self.initialized_at_ms
    }

    fn from_server_info(server: &ServerInfo) -> UseResult<Self> {
        let evidence = Self {
            protocol_version: server.protocol_version.to_string(),
            server_name: server.server_info.name.clone(),
            server_version: server.server_info.version.clone(),
            initialized_at_ms: unix_time_ms()?,
        };
        if !valid_protocol(&evidence.protocol_version)
            || !valid_server_text(&evidence.server_name)
            || !valid_server_text(&evidence.server_version)
        {
            return Err(UseError::new(
                "use.plugin.stdio_mcp.initialize_invalid",
                "The stdio MCP server returned unbounded or invalid initialize identity evidence.",
            ));
        }
        Ok(evidence)
    }
}

/// Terminal evidence returned only after the provider reports the complete
/// provider-owned process unit stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StdioMcpShutdownEvidence {
    identity: StdioMcpProcessIdentity,
    process: StdioMcpProcessObservation,
    stopped_at_ms: u64,
}

impl StdioMcpShutdownEvidence {
    /// Exact process identity that was stopped.
    pub fn identity(&self) -> &StdioMcpProcessIdentity {
        &self.identity
    }

    /// Provider's terminal process-unit observation.
    pub fn process(&self) -> &StdioMcpProcessObservation {
        &self.process
    }

    /// Host time after terminal settlement.
    pub fn stopped_at_ms(&self) -> u64 {
        self.stopped_at_ms
    }

    /// Capability observation suitable for a subsequent scoped snapshot.
    pub fn host_observation(
        &self,
        plan: &StdioMcpSessionPlan,
    ) -> UseResult<CapabilityHostSurfaceObservation> {
        self.identity.validate_against(plan)?;
        capability_observation(plan, CapabilitySurfaceObservedState::Stopped)
    }
}

/// Live initialized standard MCP session.
///
/// A detached monitor terminates the process when the exact durable grant
/// expires, disappears, is revoked or replaced, or cannot be checked.
/// Dropping the value cancels MCP and requests process-unit termination. The
/// detached lease settler continues to retain the package generation until
/// the injected provider reports exact terminal state. Call [`Self::shutdown`]
/// to receive bounded settlement evidence.
pub struct StdioMcpSession {
    plan: StdioMcpSessionPlan,
    identity: StdioMcpProcessIdentity,
    initialize: StdioMcpInitializeEvidence,
    peer: Peer<RoleClient>,
    service: Option<RunningStdioClient>,
    control: Arc<dyn StdioMcpProcessControl>,
    authorization: AuthorizationMonitor,
    settlement: LeaseSettlement,
}

impl StdioMcpSession {
    /// Exact immutable session plan.
    pub fn plan(&self) -> &StdioMcpSessionPlan {
        &self.plan
    }

    /// Exact provider process identity.
    pub fn identity(&self) -> &StdioMcpProcessIdentity {
        &self.identity
    }

    /// Successful standard MCP initialize evidence.
    pub fn initialize_evidence(&self) -> &StdioMcpInitializeEvidence {
        &self.initialize
    }

    /// Last bounded durable workspace-grant observation.
    pub fn authorization_observation(&self) -> UseResult<StdioMcpAuthorizationObservation> {
        self.authorization.observation()
    }

    /// Clone the standard RMCP client peer for schema-native MCP calls while
    /// the grant and transport remain active.
    pub fn peer(&self) -> UseResult<Peer<RoleClient>> {
        if let Some(error) = self.authorization.failure() {
            self.control.terminate();
            return Err(error);
        }
        if grant_expired(&self.plan)? {
            self.control.terminate();
            return Err(UseError::new(
                "use.plugin.stdio_mcp.grant_expired",
                "The stdio MCP workspace grant expired and the process is being stopped.",
            ));
        }
        if self.peer.is_transport_closed() {
            return Err(UseError::new(
                "use.plugin.stdio_mcp.transport_closed",
                "The supervised stdio MCP transport is closed.",
            ));
        }
        Ok(self.peer.clone())
    }

    /// Observe current liveness and project it into the scoped capability
    /// host contract.
    pub async fn host_observation(&self) -> UseResult<CapabilityHostSurfaceObservation> {
        if self.authorization.failure().is_some() {
            self.control.terminate();
            return capability_observation(&self.plan, CapabilitySurfaceObservedState::Failed);
        }
        if grant_expired(&self.plan)? {
            self.control.terminate();
            return capability_observation(&self.plan, CapabilitySurfaceObservedState::Failed);
        }
        let process = tokio::time::timeout(
            Duration::from_millis(self.plan.shutdown_timeout_ms()),
            self.control.observe(),
        )
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.observe_timeout",
                "Timed out observing the supervised stdio MCP process.",
            )
        })??;
        process.validate_against(&self.plan)?;
        let state = match process.state() {
            StdioMcpProcessState::Running if !self.peer.is_transport_closed() => {
                CapabilitySurfaceObservedState::Healthy
            }
            StdioMcpProcessState::Running | StdioMcpProcessState::Exited { .. } => {
                CapabilitySurfaceObservedState::Failed
            }
        };
        capability_observation(&self.plan, state)
    }

    /// Close MCP, wait for a graceful process exit, then request forced
    /// process-unit termination if the grace deadline expires.
    pub async fn shutdown(mut self) -> UseResult<StdioMcpShutdownEvidence> {
        let timeout = Duration::from_millis(self.plan.shutdown_timeout_ms());
        let service_result = match self.service.take() {
            Some(service) => cancel_service(service, timeout).await,
            None => Ok(()),
        };

        let process = match self.settlement.wait(timeout).await? {
            Some(process) => process,
            None => {
                self.control.terminate();
                self.settlement.wait(timeout).await?.ok_or_else(|| {
                    UseError::new(
                        "use.plugin.stdio_mcp.shutdown_timeout",
                        "The stdio MCP process unit did not stop within the graceful and forced shutdown deadlines.",
                    )
                })?
            }
        };
        process.validate_against(&self.plan)?;
        if !matches!(process.state(), StdioMcpProcessState::Exited { .. }) {
            return Err(UseError::new(
                "use.plugin.stdio_mcp.shutdown_incomplete",
                "The stdio MCP host reported a non-terminal process after shutdown.",
            ));
        }
        service_result?;
        Ok(StdioMcpShutdownEvidence {
            identity: self.identity.clone(),
            process,
            stopped_at_ms: unix_time_ms()?,
        })
    }
}

impl fmt::Debug for StdioMcpSession {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("StdioMcpSession")
            .field("plan", &self.plan)
            .field("identity", &self.identity)
            .field("initialize", &self.initialize)
            .field("authorization", &self.authorization.observation())
            .finish_non_exhaustive()
    }
}

impl Drop for StdioMcpSession {
    fn drop(&mut self) {
        if let Some(service) = &self.service {
            service.cancellation_token().cancel();
        }
        self.control.terminate();
    }
}

/// Use-owned planner and lifecycle coordinator around one explicitly injected
/// stdio compatibility host.
pub struct StdioMcpSupervisor<'a> {
    grants: &'a WorkspaceGrantStore,
    host: Arc<dyn StdioMcpHostProvider>,
}

impl<'a> StdioMcpSupervisor<'a> {
    /// Bind the durable grant store and one trusted provider. No package may
    /// select or replace this provider.
    pub fn new(grants: &'a WorkspaceGrantStore, host: Arc<dyn StdioMcpHostProvider>) -> Self {
        Self { grants, host }
    }

    /// Validate the package generation, active grant, filesystem roots, and
    /// provider evidence without spawning a process.
    pub async fn prepare(
        &self,
        lease: &StdioMcpPackageLease,
        request: StdioMcpSessionRequest,
    ) -> UseResult<PreparedStdioMcpSession> {
        request_matches_extension(&request, lease.extension())?;
        let capabilities =
            read_host_capabilities(&self.host, request.initialize_timeout_ms()).await?;
        let plan = self
            .build_plan(lease.extension(), request, capabilities)
            .await?;
        Ok(PreparedStdioMcpSession { plan })
    }

    /// Spawn and initialize exactly a prepared session while retaining the
    /// supplied package lease until provider-reported process-unit exit.
    pub async fn start(
        &self,
        prepared: PreparedStdioMcpSession,
        lease: StdioMcpPackageLease,
    ) -> UseResult<StdioMcpSession> {
        let plan = prepared.plan;
        self.revalidate_start(&plan, lease.extension()).await?;
        let capabilities = read_host_capabilities(&self.host, plan.initialize_timeout_ms()).await?;
        if capabilities != *plan.provider() {
            return Err(provider_changed(&plan, &capabilities));
        }
        self.revalidate_start(&plan, lease.extension()).await?;

        let spawned = tokio::time::timeout(
            Duration::from_millis(plan.initialize_timeout_ms()),
            self.host.spawn(&plan),
        )
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.spawn_timeout",
                "Timed out spawning the exact stdio MCP session plan.",
            )
        })?
        .map_err(|error| {
            UseError::new(
                "use.plugin.stdio_mcp.spawn_failed",
                "The selected compatibility host failed to spawn the exact stdio MCP plan.",
            )
            .with_detail("hostCode", error.code)
            .with_detail("hostMessage", error.message)
        })?;
        let (reader, writer, control) = spawned.into_parts();
        let identity = control.identity().clone();
        let mut settlement = LeaseSettlement::start(lease, plan.clone(), Arc::clone(&control));
        let mut authorization = AuthorizationMonitor::start(
            (*self.grants).clone(),
            plan.clone(),
            Arc::clone(&control),
            settlement.process_done(),
        );

        if let Err(error) = identity.validate_against(&plan) {
            return Err(cleanup_failed_start(error, None, &control, &mut settlement, &plan).await);
        }
        if let Err(error) = authorization
            .wait_initial(Duration::from_millis(plan.initialize_timeout_ms()))
            .await
        {
            return Err(cleanup_failed_start(error, None, &control, &mut settlement, &plan).await);
        }
        let after_spawn =
            match read_host_capabilities(&self.host, plan.initialize_timeout_ms()).await {
                Ok(capabilities) => capabilities,
                Err(error) => {
                    return Err(
                        cleanup_failed_start(error, None, &control, &mut settlement, &plan).await,
                    )
                }
            };
        if after_spawn != *plan.provider() {
            let error = provider_changed(&plan, &after_spawn);
            return Err(cleanup_failed_start(error, None, &control, &mut settlement, &plan).await);
        }

        let client_info = ClientInfo {
            protocol_version: ProtocolVersion::LATEST,
            capabilities: ClientCapabilities::default(),
            client_info: Implementation {
                name: "a3s-use-stdio-mcp-supervisor".to_string(),
                title: Some("A3S Use stdio MCP Supervisor".to_string()),
                version: env!("CARGO_PKG_VERSION").to_string(),
                icons: None,
                website_url: None,
            },
        };
        let transport = BoundedStdioTransport::<RoleClient>::new(reader, writer);
        let service = match tokio::time::timeout(
            Duration::from_millis(plan.initialize_timeout_ms()),
            client_info.serve(transport),
        )
        .await
        {
            Err(_) => {
                let error = UseError::new(
                    "use.plugin.stdio_mcp.initialize_timeout",
                    "Timed out performing the bounded standard MCP initialize handshake.",
                );
                return Err(
                    cleanup_failed_start(error, None, &control, &mut settlement, &plan).await,
                );
            }
            Ok(Err(_)) => {
                let error = UseError::new(
                    "use.plugin.stdio_mcp.initialize_failed",
                    "The bounded standard MCP initialize handshake failed.",
                );
                return Err(
                    cleanup_failed_start(error, None, &control, &mut settlement, &plan).await,
                );
            }
            Ok(Ok(service)) => service,
        };
        let server_info = match service.peer_info().cloned() {
            Some(server_info) => server_info,
            None => {
                let error = UseError::new(
                    "use.plugin.stdio_mcp.initialize_failed",
                    "The stdio MCP session completed initialization without server identity.",
                );
                return Err(cleanup_failed_start(
                    error,
                    Some(service),
                    &control,
                    &mut settlement,
                    &plan,
                )
                .await);
            }
        };
        let initialize = match StdioMcpInitializeEvidence::from_server_info(&server_info) {
            Ok(initialize) => initialize,
            Err(error) => {
                return Err(cleanup_failed_start(
                    error,
                    Some(service),
                    &control,
                    &mut settlement,
                    &plan,
                )
                .await)
            }
        };
        let process = match tokio::time::timeout(
            Duration::from_millis(plan.initialize_timeout_ms()),
            control.observe(),
        )
        .await
        {
            Err(_) => {
                let error = UseError::new(
                    "use.plugin.stdio_mcp.observe_timeout",
                    "Timed out confirming the initialized stdio MCP process identity.",
                );
                return Err(cleanup_failed_start(
                    error,
                    Some(service),
                    &control,
                    &mut settlement,
                    &plan,
                )
                .await);
            }
            Ok(Err(error)) => {
                return Err(cleanup_failed_start(
                    error,
                    Some(service),
                    &control,
                    &mut settlement,
                    &plan,
                )
                .await)
            }
            Ok(Ok(process)) => process,
        };
        if let Err(error) = process.validate_against(&plan) {
            return Err(cleanup_failed_start(
                error,
                Some(service),
                &control,
                &mut settlement,
                &plan,
            )
            .await);
        }
        if !matches!(process.state(), StdioMcpProcessState::Running)
            || service.is_transport_closed()
        {
            let error = UseError::new(
                "use.plugin.stdio_mcp.exited_during_startup",
                "The stdio MCP process or transport exited during initialization.",
            );
            return Err(cleanup_failed_start(
                error,
                Some(service),
                &control,
                &mut settlement,
                &plan,
            )
            .await);
        }
        if let Some(error) = authorization.failure() {
            return Err(cleanup_failed_start(
                error,
                Some(service),
                &control,
                &mut settlement,
                &plan,
            )
            .await);
        }

        let peer = service.peer().clone();
        Ok(StdioMcpSession {
            plan,
            identity,
            initialize,
            peer,
            service: Some(service),
            control,
            authorization,
            settlement,
        })
    }

    async fn build_plan(
        &self,
        extension: &InstalledExtension,
        request: StdioMcpSessionRequest,
        capabilities: super::StdioMcpHostCapabilities,
    ) -> UseResult<StdioMcpSessionPlan> {
        let catalog = extension.plan_ready_catalog()?;
        let package_digest = receipt_package_digest(extension)?;
        request
            .roots()
            .validate_against_package_root(&extension.receipt.package_root)?;
        let (executable, args) = stdio_launch(extension, request.surface_id())?;
        let grant = resolve_grant(
            self.grants,
            &request,
            &package_digest,
            &catalog.record.permission_ceiling,
        )
        .await?;
        let permission = surface_permission(&grant, request.surface_id())?.clone();
        capabilities.validate_for(&permission, &grant.grant.authority)?;
        StdioMcpSessionPlan::from_input(StdioMcpPlanInput {
            request,
            package_digest,
            receipt_digest: extension.receipt.descriptor_digest()?,
            catalog_digest: catalog.descriptor_digest()?,
            manifest_digest: prefixed_digest(&extension.receipt.manifest_sha256)?,
            grant_revision: grant.revision,
            grant_digest: grant.grant_digest,
            grant_authority: grant.grant.authority,
            grant_expires_at_ms: grant.grant.expires_at_ms,
            permission_ceiling_digest: catalog.record.permission_ceiling_digest.clone(),
            package_root: extension.receipt.package_root.clone(),
            executable,
            args,
            permission,
            provider: capabilities,
        })
    }

    async fn revalidate_start(
        &self,
        plan: &StdioMcpSessionPlan,
        extension: &InstalledExtension,
    ) -> UseResult<()> {
        validate_extension_against_plan(extension, plan)?;
        let catalog = extension.plan_ready_catalog()?;
        let request = StdioMcpSessionRequest::new(
            plan.session_id(),
            plan.scope_id(),
            plan.package_id(),
            &plan.surface().id,
            plan.roots().clone(),
        )?
        .with_timeouts(plan.initialize_timeout_ms(), plan.shutdown_timeout_ms())?
        .with_authorization_recheck_interval(plan.authorization_recheck_interval_ms())?;
        let grant = resolve_grant(
            self.grants,
            &request,
            plan.package_digest(),
            &catalog.record.permission_ceiling,
        )
        .await?;
        if grant.revision != plan.grant_revision()
            || grant.grant_digest != plan.grant_digest()
            || surface_permission(&grant, &plan.surface().id)? != plan.permission()
        {
            return Err(UseError::new(
                "use.plugin.stdio_mcp.grant_changed",
                "The active workspace grant changed after the stdio MCP session was prepared.",
            ));
        }
        Ok(())
    }
}

async fn cleanup_failed_start(
    primary: UseError,
    service: Option<RunningStdioClient>,
    control: &Arc<dyn StdioMcpProcessControl>,
    settlement: &mut LeaseSettlement,
    plan: &StdioMcpSessionPlan,
) -> UseError {
    let timeout = Duration::from_millis(plan.shutdown_timeout_ms());
    let service_cleanup = match service {
        Some(service) => cancel_service(service, timeout).await,
        None => Ok(()),
    };
    control.terminate();
    let process_cleanup = settlement.wait(timeout).await.and_then(|result| {
        result.ok_or_else(|| {
            UseError::new(
                "use.plugin.stdio_mcp.cleanup_timeout",
                "The stdio MCP process unit did not settle after startup failure.",
            )
        })
    });
    attach_cleanup(
        attach_cleanup(primary, service_cleanup),
        process_cleanup.map(drop),
    )
}

async fn cancel_service(service: RunningStdioClient, timeout: Duration) -> UseResult<()> {
    let reason = tokio::time::timeout(timeout, service.cancel())
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.mcp_close_timeout",
                "Timed out closing the standard MCP stdio session.",
            )
        })?
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.mcp_close_failed",
                "The standard MCP stdio session worker failed during close.",
            )
        })?;
    match reason {
        QuitReason::Cancelled | QuitReason::Closed => Ok(()),
        QuitReason::JoinError(_) => Err(UseError::new(
            "use.plugin.stdio_mcp.mcp_close_failed",
            "The standard MCP stdio session worker joined with a failure.",
        )),
    }
}

fn validate_extension_against_plan(
    extension: &InstalledExtension,
    plan: &StdioMcpSessionPlan,
) -> UseResult<()> {
    let catalog = extension.plan_ready_catalog()?;
    let (executable, args) = stdio_launch(extension, &plan.surface().id)?;
    if extension.receipt.package_id != plan.package_id()
        || receipt_package_digest(extension)? != plan.package_digest()
        || extension.receipt.descriptor_digest()? != plan.receipt_digest()
        || catalog.descriptor_digest()? != plan.catalog_digest()
        || prefixed_digest(&extension.receipt.manifest_sha256)? != plan.manifest_digest()
        || catalog.record.permission_ceiling_digest != plan.permission_ceiling_digest()
        || extension.receipt.package_root != plan.package_root()
        || executable != plan.executable()
        || args != plan.args()
    {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.package_changed",
            "The active package generation no longer matches the prepared stdio MCP plan.",
        ));
    }
    Ok(())
}

fn request_matches_extension(
    request: &StdioMcpSessionRequest,
    extension: &InstalledExtension,
) -> UseResult<()> {
    if request.package_id() != extension.receipt.package_id || !extension.receipt.enabled {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.package_mismatch",
            "The stdio MCP request does not match an enabled leased package generation.",
        ));
    }
    if extension.manifest.schema_version != 3 {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.schema_unsupported",
            "Only named schema-v3 stdio MCP surfaces use the scoped supervisor.",
        ));
    }
    Ok(())
}

fn stdio_launch(
    extension: &InstalledExtension,
    surface_id: &str,
) -> UseResult<(std::path::PathBuf, Vec<String>)> {
    let surface = extension
        .manifest
        .mcp_servers
        .iter()
        .find(|surface| surface.id == surface_id)
        .ok_or_else(|| {
            UseError::new(
                "use.plugin.stdio_mcp.surface_missing",
                "The leased package does not declare the requested named MCP surface.",
            )
        })?;
    let PluginMcpLaunch::Stdio { executable, args } = &surface.launch else {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.transport_mismatch",
            "A Streamable HTTP MCP Service cannot be started by the stdio compatibility host.",
        ));
    };
    Ok((
        extension.receipt.package_root.join(executable),
        args.clone(),
    ))
}

async fn resolve_grant(
    grants: &WorkspaceGrantStore,
    request: &StdioMcpSessionRequest,
    package_digest: &str,
    ceiling: &a3s_use_core::PluginPermissionCeiling,
) -> UseResult<WorkspaceGrantReceipt> {
    let record = grants
        .observe(request.scope_id(), request.package_id(), package_digest)
        .await?
        .ok_or_else(|| {
            UseError::new(
                "use.plugin.stdio_mcp.grant_missing",
                "The stdio MCP surface has no active package-generation workspace grant.",
            )
        })?;
    let StoredWorkspaceGrant::Granted(receipt) = record else {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.grant_revoked",
            "The stdio MCP package-generation workspace grant is revoked.",
        ));
    };
    receipt
        .grant
        .validate_active_against(ceiling, unix_time_ms()?)?;
    Ok(receipt)
}

fn surface_permission<'a>(
    grant: &'a WorkspaceGrantReceipt,
    surface_id: &str,
) -> UseResult<&'a SurfacePermissionCeiling> {
    grant
        .grant
        .permissions
        .surfaces
        .iter()
        .find(|permission| {
            permission.surface.kind == PluginSurfaceKind::Mcp && permission.surface.id == surface_id
        })
        .ok_or_else(|| {
            UseError::new(
                "use.plugin.stdio_mcp.permission_missing",
                "The active workspace grant does not authorize the named stdio MCP surface.",
            )
        })
}

fn receipt_package_digest(extension: &InstalledExtension) -> UseResult<String> {
    extension
        .receipt
        .package_sha256
        .as_deref()
        .ok_or_else(|| {
            UseError::new(
                "use.plugin.stdio_mcp.package_unverified",
                "A supervised stdio MCP surface requires an immutable expanded-package digest.",
            )
        })
        .and_then(prefixed_digest)
}

fn prefixed_digest(value: &str) -> UseResult<String> {
    let digest = value
        .strip_prefix("sha256:")
        .unwrap_or(value)
        .to_ascii_lowercase();
    if digest.len() != 64
        || !digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(UseError::new(
            "use.plugin.stdio_mcp.package_unverified",
            "A stdio MCP package contains a noncanonical SHA-256 identity.",
        ));
    }
    Ok(format!("sha256:{digest}"))
}

fn provider_changed(
    plan: &StdioMcpSessionPlan,
    observed: &super::StdioMcpHostCapabilities,
) -> UseError {
    UseError::new(
        "use.plugin.stdio_mcp.provider_changed",
        "The selected stdio MCP host changed after the session was prepared.",
    )
    .with_detail("plannedProviderId", plan.provider().provider_id())
    .with_detail("observedProviderId", observed.provider_id())
    .with_detail(
        "plannedCapabilityDigest",
        plan.provider().capability_digest(),
    )
    .with_detail("observedCapabilityDigest", observed.capability_digest())
}

async fn read_host_capabilities(
    host: &Arc<dyn StdioMcpHostProvider>,
    timeout_ms: u64,
) -> UseResult<super::StdioMcpHostCapabilities> {
    tokio::time::timeout(Duration::from_millis(timeout_ms), host.capabilities())
        .await
        .map_err(|_| {
            UseError::new(
                "use.plugin.stdio_mcp.capability_timeout",
                "Timed out reading the selected stdio MCP host capabilities.",
            )
        })?
}

fn capability_observation(
    plan: &StdioMcpSessionPlan,
    state: CapabilitySurfaceObservedState,
) -> UseResult<CapabilityHostSurfaceObservation> {
    CapabilityHostSurfaceObservation::new(
        plan.package_id(),
        plan.package_digest(),
        plan.surface().clone(),
        CapabilityHostSurfaceOwner::McpHost,
        state,
    )
}

fn attach_cleanup(primary: UseError, cleanup: UseResult<()>) -> UseError {
    match cleanup {
        Ok(()) => primary,
        Err(cleanup) => primary
            .with_detail("cleanupCode", cleanup.code)
            .with_detail("cleanupMessage", cleanup.message),
    }
}

fn grant_expired(plan: &StdioMcpSessionPlan) -> UseResult<bool> {
    match plan.grant_expires_at_ms() {
        Some(expires_at_ms) => Ok(unix_time_ms()? >= expires_at_ms),
        None => Ok(false),
    }
}
