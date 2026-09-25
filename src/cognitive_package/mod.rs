//! Production composition for schema-v3 cognitive-package graphs.
//!
//! Registry endpoints and trust roots are supplied by the embedding host.
//! This module owns dependency-lock planning and package-level lifecycle
//! composition; Tool, MCP, Skill, UI, and OKF remain contributions inside one
//! immutable package generation.

mod control_authority;
mod diagnostic;
mod diagnostic_history;
#[cfg(test)]
mod diagnostic_history_tests;
mod download_attempt;
#[cfg(test)]
mod download_attempt_tests;
mod embedded;
mod enablement;
mod enablement_plan;
mod enablement_store;
mod grant;
mod host_manager;
mod host_snapshot;
mod host_store;
mod hosts;
mod install;
mod mutation_lock;
mod native_provider;
mod observation_snapshot;
mod plan;
mod planning_attempt_io;
mod prepare_graph;
mod provider_plan;
mod registry_access;
mod resolution_attempt;
#[cfg(test)]
mod resolution_attempt_tests;
mod reviewed_authorization;
mod store;
mod uninstall;
mod upgrade;
mod upgrade_validation;

use a3s_use_core::{
    InstallationId, LockedPluginPackage, PlanScope, PluginOperationAction,
    PluginOperationPlanEnvelope, PluginPackageLock, UseError, UseResult,
    VerifiedPluginCatalogRecord,
};
use a3s_use_extension::{
    ExtensionLifecyclePackage, ExtensionManifest, ExtensionRegistry, InstalledExtension,
};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Arc;

use download_attempt::PackageDownloadAttemptStore;
#[cfg(test)]
pub(crate) use observation_snapshot::planning_observation_snapshot_fixtures;
pub(crate) use observation_snapshot::{
    validate_planning_observation_snapshot_record, PlanningObservationSnapshotRecord,
    PlanningObservationSnapshotRecordKind,
};
use resolution_attempt::PackageResolutionAttemptStore;
pub(crate) use store::{
    acquire_existing_package_graph_lock_shared, inspect_pending_artifact_references_locked,
    PendingPackageGraphArtifactReferences,
};
#[cfg(test)]
pub(crate) use store::InstallationSnapshotStore;
use store::{PackageGraphOperationPhase, PendingPackageGraphOperation};

pub(crate) use control_authority::{
    open_control_lifecycle, require_control_runtime_readiness_for_publications,
};
pub use diagnostic::{
    PluginDownloadAttemptDiagnostic, PluginDownloadAttemptPhase, PluginDownloadDiagnosticStatus,
    PluginDownloadTargetDiagnostic, PluginDownloadTargetDiagnosticStatus,
    PluginGrantDiagnosticStatus, PluginGrantOperationDiagnostic,
    PluginLifecycleDrainDiagnosticStatus, PluginLifecycleOperationSummary,
    PluginLifecyclePublicationDiagnosticStatus, PluginOperationConfirmationDiagnosticStatus,
    PluginOperationDiagnostic, PluginOperationDiagnosticPhase, PluginOperationHistoryDiagnostic,
    PluginOperationRecoveryGuidance, PluginOperationSourceDiagnostic,
    PluginPendingDownloadAttemptDiagnostic, PluginPendingOperationDiagnostic,
    PluginPendingResolutionAttemptDiagnostic, PluginPlanningTargetDiagnostic,
    PluginProviderDiagnosticReadiness, PluginProviderOperationDiagnostic,
    PluginRegistryCutoverDiagnostic, PluginRegistryCutoverDiagnosticStatus,
    PluginRegistryOperationDiagnostic, PluginRegistryResolutionAccess,
    PluginRegistryResolutionDiagnostic, PluginRegistryResolutionRole,
    PluginRegistryResolutionStatus, PluginResolutionAttemptDiagnostic,
    PluginResolutionAttemptPhase, PluginResolutionDiagnosticStatus,
    PluginRetainedOperationDiagnostic, PluginRetainedOperationOutcome,
    MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES, MAX_PLUGIN_OPERATION_HISTORY_BYTES,
    MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS, MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES,
    PLUGIN_DOWNLOAD_ATTEMPT_DIAGNOSTIC_SCHEMA, PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA,
    PLUGIN_OPERATION_HISTORY_DIAGNOSTIC_SCHEMA, PLUGIN_RESOLUTION_ATTEMPT_DIAGNOSTIC_SCHEMA,
};
pub use embedded::{
    CognitiveCapabilityEvidence, CognitiveCapabilityLease, CognitiveCatalogPageCursor,
    CognitiveCatalogSearchResult, CognitiveRegistryAccess,
};
pub use enablement::{
    CognitivePackageEnablementRequest, CognitivePackageEnablementResult,
    COGNITIVE_PACKAGE_ENABLEMENT_REQUEST_SCHEMA, COGNITIVE_PACKAGE_ENABLEMENT_RESULT_SCHEMA,
};
pub use enablement_plan::{
    CognitivePackageEnablementDraft, CognitivePackageEnablementPlanResult,
    CognitivePackageEnablementPlanStatus, CognitivePackageEnablementPreparation,
    COGNITIVE_PACKAGE_ENABLEMENT_PLAN_RESULT_SCHEMA,
};
#[cfg(test)]
pub(crate) use grant::reconstruct_planned_workspace_grants;
pub(crate) use grant::PlannedWorkspaceGrantOperation;
pub use grant::{
    bind_cognitive_package_grant_impacts, bind_cognitive_package_grants,
    reconstruct_cognitive_package_grants, CognitivePackageAuthorizationEvidence,
    CognitivePackageAuthorizationProvider, CognitivePackageGrantPlan,
    StandaloneCognitivePackageAuthorizationProvider,
};
pub use host_manager::CognitivePackageHostManager;
#[cfg(test)]
pub(crate) use host_snapshot::{
    host_projection_snapshot_fixture_sources, write_host_projection_no_change_fixture,
    write_host_projection_snapshot_fixture, HostProjectionSnapshotFixtureOutcome,
};
pub(crate) use host_snapshot::{
    scan_host_projection_snapshot, validate_host_projection_snapshot_record,
    validate_host_projection_snapshot_set, HostProjectionRestoreIndexBuilder,
    HostProjectionSnapshotRecord, HostProjectionSnapshotRecordKind, HostProjectionSnapshotRequest,
    HOST_PROJECTION_SNAPSHOT_MAX_RECORD_BYTES,
};
pub use hosts::{
    ManagedCognitivePackageLifecycleFactory, StandaloneCognitivePackageLifecycleFactory,
    A3S_FLOW_NATIVE_TS_COMPILER_ENV,
};
/// Host-owned Control Runtime Service readiness port for product injection.
pub use crate::control_store::{
    ControlRuntimeMcpReadiness, ControlRuntimeServiceReadinessPort,
};
pub(crate) use install::verify_expected_lock;
pub use native_provider::plan_native_provider_evidence;
pub use provider_plan::{
    bind_cognitive_package_provider_plan, plan_cognitive_package_provider_generations,
    plan_cognitive_package_providers, BoundCognitivePackageProviderPlan,
    CognitivePackageProviderPlan,
};
pub use reviewed_authorization::ReviewedCognitivePackageAuthorizationProvider;

/// Version of the A3S Use package engine enforcing cognitive-package host
/// compatibility.
pub const COGNITIVE_PACKAGE_HOST_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Canonical target name used by signed cognitive-package catalogs and locks.
pub fn cognitive_package_host_target() -> UseResult<String> {
    current_host_target()
}

/// Shared package manager used by the standalone CLI and embedding A3S hosts.
/// Registry selection remains host configuration and is passed per operation.
#[derive(Clone)]
pub struct CognitivePackageManager {
    registry: ExtensionRegistry,
    lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
    authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    control: Arc<tokio::sync::OnceCell<OpenedControl>>,
}

/// One Control open for this manager, recording whether signed description
/// trust was injected at open time (OnceCell cannot upgrade afterward).
struct OpenedControl {
    lifecycle: crate::control_store::ProductionControlLifecycle,
    signed_description_trust: bool,
}

impl std::fmt::Debug for CognitivePackageManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CognitivePackageManager")
            .field("registry", &self.registry)
            .field("installation", self.registry.installation())
            .field("lifecycle", &self.lifecycle.name())
            .field("authorization", &self.authorization.name())
            .finish()
    }
}

/// Declared lifecycle surfaces one host factory can actually compose.
///
/// Unsupported surfaces must stay `false`. Callers negotiate against this
/// report instead of assuming default trait methods imply readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CognitiveLifecycleSupport {
    pub tool_task_executable: bool,
    pub tool_task_runtime: bool,
    pub tool_service_runtime: bool,
    pub mcp_stdio: bool,
    pub mcp_streamable_http: bool,
    pub skill: bool,
    pub okf: bool,
    pub flow: bool,
    pub ui: bool,
}

/// Host composition boundary for one package-owned lifecycle saga.
///
/// A3S Code, A3S OS, or another embedding host injects exact Runtime, Gateway,
/// Knowledge, Skill, and UI adapters here. The standalone implementation is
/// deliberately narrower and fails closed for surfaces whose owner is absent.
///
/// No default method may pretend an unsupported planning, retirement, or
/// enablement path is available. Factories must declare
/// [`Self::supported_lifecycle`] and assemble a
/// [`crate::plugin_lifecycle::LifecycleProviderSet`]. The Use engine alone
/// turns that set into a [`crate::plugin_lifecycle::PluginLifecycleCoordinator`].
pub trait CognitivePackageLifecycleFactory: Send + Sync {
    fn name(&self) -> &'static str;

    /// Exact surfaces this factory can compose. Negotiation and validation
    /// must read this report; absence of a surface is fail-closed.
    fn supported_lifecycle(&self) -> CognitiveLifecycleSupport;

    /// Optional absolute path to the reviewed A3S Flow native TypeScript
    /// compiler. Control uses this to compose the Flow effect owner; absence
    /// keeps Flow surfaces fail-closed under production drain.
    fn flow_compiler_binary(&self) -> Option<&std::path::Path>;

    /// Optional Control-shaped Runtime Service readiness port for managed
    /// hosts that own live Gateway/Runtime Service bindings.
    ///
    /// Standalone factories return `None` (opaque `gateway:` minting). Managed
    /// hosts inject a [`ControlRuntimeServiceReadinessPort`]
    /// via the factory builder so Control effects bind real endpoints.
    fn control_runtime_readiness(
        &self,
    ) -> Option<std::sync::Arc<dyn ControlRuntimeServiceReadinessPort>> {
        None
    }

    /// Process-local Runtime clients Control drain uses to reconnect committed
    /// providers. Managed hosts must return the same registry that owns their
    /// Box/A3S providers; standalone defaults to an empty registry.
    fn runtime_client_registry(&self) -> std::sync::Arc<a3s_runtime::RuntimeClientRegistry> {
        std::sync::Arc::new(a3s_runtime::RuntimeClientRegistry::new())
    }

    /// Immutable Runtime plan publications that must be admitted before Control
    /// commit when this factory owns managed Tool/MCP surfaces.
    ///
    /// Standalone and skill-only factories return an empty set. Managed hosts
    /// that configured a [`crate::plugin_runtime::RuntimeProviderSelection`]
    /// return its deterministic publications.
    fn runtime_plan_publications(
        &self,
    ) -> a3s_use_core::UseResult<Vec<crate::plugin_runtime::RuntimeSurfacePlanPublication>> {
        Ok(Vec::new())
    }

    fn validate_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()>;

    /// Validate host-owned lifecycle availability before Runtime provider
    /// preflight. Managed hosts use a provider-neutral check so an enablement
    /// draft can be created before its exact Runtime selection exists.
    fn validate_manifest_for_planning(&self, manifest: &ExtensionManifest) -> UseResult<()>;

    /// Validate adapters needed only to retire an already receipt-bound
    /// generation. Runtime provider ownership is resolved from durable binding
    /// receipts, so managed hosts must not require a new activation selection.
    fn validate_manifest_for_retirement(&self, manifest: &ExtensionManifest) -> UseResult<()>;

    fn install_providers(
        &self,
        registry: ExtensionRegistry,
        candidate: ExtensionLifecyclePackage,
        package_root: PathBuf,
    ) -> UseResult<crate::plugin_lifecycle::LifecycleProviderSet>;

    fn published_install_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<crate::plugin_lifecycle::LifecycleProviderSet>;

    fn uninstall_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<crate::plugin_lifecycle::LifecycleProviderSet>;

    /// Compose enable/disable over an already committed immutable generation.
    ///
    /// Enablement has no package commit or removal checkpoint, but it must use
    /// the exact same Runtime, Gateway, Knowledge, Flow, Skill, UI, and
    /// capability hosts as install and uninstall.
    fn enablement_providers(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<crate::plugin_lifecycle::LifecycleProviderSet>;
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitivePackageInstallResult {
    pub changed: bool,
    pub root: InstalledExtension,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PluginOperationPlanEnvelope>,
    pub installed_packages: Vec<String>,
    pub retained_packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitivePackageUninstallResult {
    pub changed: bool,
    pub root_package_id: String,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    pub plan: PluginOperationPlanEnvelope,
    pub removed_packages: Vec<String>,
    pub retained_packages: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CognitivePackageUpgradeResult {
    pub changed: bool,
    pub root: InstalledExtension,
    pub prior_package_lock: PluginPackageLock,
    pub package_lock: PluginPackageLock,
    pub package_lock_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan: Option<PluginOperationPlanEnvelope>,
    pub added_packages: Vec<String>,
    pub replaced_packages: Vec<String>,
    pub removed_packages: Vec<String>,
    pub retained_packages: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum InstallDisposition {
    Add,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UninstallDisposition {
    Remove,
    Retain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum UpgradeDisposition {
    Add,
    Replace,
    Remove,
    Retain,
}

impl CognitivePackageManager {
    pub fn from_env(installation: InstallationId) -> UseResult<Self> {
        Self::with_lifecycle(
            ExtensionRegistry::from_env(installation)?,
            Arc::new(hosts::StandaloneCognitivePackageLifecycleFactory::from_env()?),
        )
    }

    pub fn new(registry: ExtensionRegistry) -> UseResult<Self> {
        Self::with_lifecycle_and_authorization(
            registry,
            Arc::new(hosts::StandaloneCognitivePackageLifecycleFactory::default()),
            Arc::new(StandaloneCognitivePackageAuthorizationProvider),
        )
    }

    pub fn with_lifecycle(
        registry: ExtensionRegistry,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
    ) -> UseResult<Self> {
        Self::with_lifecycle_and_authorization(
            registry,
            lifecycle,
            Arc::new(StandaloneCognitivePackageAuthorizationProvider),
        )
    }

    pub fn with_authorization(
        registry: ExtensionRegistry,
        authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    ) -> UseResult<Self> {
        Self::with_lifecycle_and_authorization(
            registry,
            Arc::new(hosts::StandaloneCognitivePackageLifecycleFactory::default()),
            authorization,
        )
    }

    pub fn with_lifecycle_and_authorization(
        registry: ExtensionRegistry,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
        authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    ) -> UseResult<Self> {
        registry.installation().validate()?;
        // Fail closed on legacy authority leaves at construction; Control is
        // the only mutable authority (no stored dual-path selector).
        control_authority::select_installation_authority(
            &registry.paths().installation_state_root(),
        )?;
        Ok(Self {
            registry,
            lifecycle,
            authorization,
            control: Arc::new(tokio::sync::OnceCell::new()),
        })
    }

    /// Construct an embedding-host manager bound to one exact plan scope.
    ///
    /// The Registry already owns an explicit installation. Managed hosts use
    /// this entry point to prove their advertised scope is that same identity,
    /// so equal textual IDs in different kinds cannot be substituted.
    pub fn with_plan_scope_lifecycle_and_authorization(
        registry: ExtensionRegistry,
        scope: PlanScope,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
        authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    ) -> UseResult<Self> {
        scope.validate()?;
        if registry.installation() != &scope {
            return Err(package_manager_error(
                "use.plugin.package_installation_mismatch",
                "The cognitive-package manager scope differs from its Registry installation.",
            ));
        }
        Self::with_lifecycle_and_authorization(registry, lifecycle, authorization)
    }

    pub fn registry(&self) -> &ExtensionRegistry {
        &self.registry
    }

    pub fn scope(&self) -> &PlanScope {
        self.registry.installation()
    }

    pub fn lifecycle(&self) -> &dyn CognitivePackageLifecycleFactory {
        self.lifecycle.as_ref()
    }

    pub fn authorization(&self) -> &dyn CognitivePackageAuthorizationProvider {
        self.authorization.as_ref()
    }

    pub(crate) async fn ensure_control(
        &self,
    ) -> UseResult<&crate::control_store::ProductionControlLifecycle> {
        // One Control open path: when a TrustedRegistry is configured, load
        // signed description trust even for install/plan/observe callers.
        // Opaque-first unsigned open used to poison later Gateway serve.
        self.ensure_control_for_registry_lifecycle(None, CognitiveRegistryAccess::Cached)
            .await
    }

    /// Open Control, injecting a Registry/TUF-verified description trust store.
    ///
    /// Pass `registry_name` from product CLI (`--registry-name`) or `None` to
    /// use the configured default. Empty Registry source configuration keeps the
    /// unsigned preview projector. A selected or default Registry always loads
    /// `capability/description-trust-store-v1.json` through the signed target
    /// path; fixture keys are never accepted here.
    ///
    /// Callers that already opened Control without signed trust cannot upgrade
    /// the projector afterward; this entrypoint fail-closes instead of serving
    /// an unsigned Gateway under a Registry that requires verification.
    pub async fn ensure_control_for_registry(
        &self,
        registry_name: Option<&str>,
        access: CognitiveRegistryAccess,
    ) -> UseResult<()> {
        self.ensure_control_for_registry_lifecycle(registry_name, access)
            .await
            .map(|_| ())
    }

    pub(crate) async fn ensure_control_for_registry_lifecycle(
        &self,
        registry_name: Option<&str>,
        access: CognitiveRegistryAccess,
    ) -> UseResult<&crate::control_store::ProductionControlLifecycle> {
        let publications = self.lifecycle.runtime_plan_publications()?;
        control_authority::require_control_runtime_readiness_for_publications(
            self.lifecycle.control_runtime_readiness().as_ref(),
            &publications,
        )?;
        let signed_trust = control_authority::load_signed_description_trust_for_control(
            self.registry.paths(),
            registry_name,
            access,
        )
        .await?;
        let require_signed = signed_trust.is_some();
        if let Some(opened) = self.control.get() {
            deny_unsigned_control_when_signed_trust_required(
                opened.signed_description_trust,
                require_signed,
            )?;
            return Ok(&opened.lifecycle);
        }
        let opened = self
            .control
            .get_or_try_init(|| async {
                let lifecycle = control_authority::open_control_lifecycle_with_host_ports(
                    self.registry.paths(),
                    self.lifecycle.runtime_client_registry(),
                    self.lifecycle.flow_compiler_binary(),
                    signed_trust,
                    self.lifecycle.control_runtime_readiness(),
                )
                .await?;
                Ok(OpenedControl {
                    lifecycle,
                    signed_description_trust: require_signed,
                })
            })
            .await?;
        deny_unsigned_control_when_signed_trust_required(
            opened.signed_description_trust,
            require_signed,
        )?;
        Ok(&opened.lifecycle)
    }

    /// Prefer an already-opened Control cell (for example after
    /// [`Self::ensure_control_for_registry`]); otherwise open with default
    /// cached Registry trust when sources are configured.
    async fn require_control(
        &self,
    ) -> UseResult<&crate::control_store::ProductionControlLifecycle> {
        if let Some(opened) = self.control.get() {
            return Ok(&opened.lifecycle);
        }
        self.ensure_control().await
    }

    /// Open the durable published Capability Gateway for a long-lived host.
    ///
    /// Returns `None` when Control has no published catalog. Product CLI and
    /// embedding hosts that retain the session across graph apply must call
    /// [`Self::attach_retained_gateway_cutover`] so production drain activates
    /// the endpoint after CapabilityCutover (before prior-generation
    /// Remove/Prepare). Out-of-band publication advance uses
    /// [`Self::watch_and_reconcile_published_capability_gateway`]. Call
    /// [`Self::ensure_control_for_registry`] first when signed Tool description
    /// trust is required.
    #[cfg(feature = "mcp")]
    pub async fn open_published_capability_gateway(
        &self,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<crate::capability_gateway::CapabilityGatewaySessionFactory>> {
        self.require_control()
            .await?
            .open_published_capability_gateway(options)
            .await
    }

    /// Serve the published Capability Gateway over stdio (one-shot process).
    #[cfg(feature = "mcp")]
    pub async fn serve_published_capability_gateway_stdio(
        &self,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<()> {
        self.require_control()
            .await?
            .serve_published_capability_gateway_stdio(options)
            .await
    }

    /// Serve a long-lived HTTP Gateway with Control reconcile and shutdown drain.
    #[cfg(feature = "mcp")]
    pub async fn serve_published_capability_gateway_streamable_http(
        &self,
        listener: tokio::net::TcpListener,
        config: crate::capability_gateway::CapabilityGatewayHttpConfig,
        shutdown: tokio_util::sync::CancellationToken,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<()> {
        self.require_control()
            .await?
            .serve_published_capability_gateway_streamable_http(
                listener, config, shutdown, options,
            )
            .await
    }

    /// Build the graph-lifecycle cutover activation for one retained Gateway.
    ///
    /// Prefer [`Self::attach_retained_gateway_cutover`] for Control production
    /// apply. Same-process hosts that still drive
    /// [`crate::plugin_lifecycle::PluginPackageGraphLifecycleCoordinator`] may
    /// attach the returned activation via
    /// [`crate::plugin_lifecycle::PluginPackageGraphLifecycleCoordinator::with_capability_cutover_activation`].
    #[cfg(feature = "mcp")]
    pub async fn gateway_cutover_activation(
        &self,
        session: crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<Arc<dyn crate::plugin_lifecycle::PluginGraphCapabilityCutoverActivation>>
    {
        Ok(self
            .require_control()
            .await?
            .gateway_cutover_activation(session, options))
    }

    /// Retain a live Gateway across subsequent Control production applies.
    ///
    /// Production drain activates the session after CapabilityCutover so
    /// prior-generation leases release before Remove/Prepare. Clear with
    /// [`Self::clear_retained_gateway_cutover`] when the host drops the session.
    #[cfg(feature = "mcp")]
    pub async fn attach_retained_gateway_cutover(
        &self,
        session: crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
    ) -> UseResult<()> {
        self.require_control()
            .await?
            .attach_retained_gateway_cutover(session, options);
        Ok(())
    }

    /// Drop the retained Gateway cutover attachment.
    #[cfg(feature = "mcp")]
    pub async fn clear_retained_gateway_cutover(&self) -> UseResult<()> {
        self.require_control()
            .await?
            .clear_retained_gateway_cutover();
        Ok(())
    }

    /// Watch Control and reconcile one retained Gateway until `shutdown`.
    #[cfg(feature = "mcp")]
    pub async fn watch_and_reconcile_published_capability_gateway(
        &self,
        session: &crate::capability_gateway::CapabilityGatewaySessionFactory,
        options: crate::capability_gateway::CapabilityGatewayCompositionOptions,
        shutdown: &tokio_util::sync::CancellationToken,
        poll_interval: std::time::Duration,
    ) -> UseResult<()> {
        self.require_control()
            .await?
            .watch_and_reconcile_published_capability_gateway(
                session,
                options,
                shutdown,
                poll_interval,
            )
            .await
    }

    /// Drain a retained Gateway and retain only Control-selected payloads.
    #[cfg(feature = "mcp")]
    pub async fn drain_and_retain_published_capability_gateway(
        &self,
        session: &crate::capability_gateway::CapabilityGatewaySessionFactory,
        drain_timeout: std::time::Duration,
    ) -> UseResult<()> {
        self.require_control()
            .await?
            .drain_published_capability_gateway_for_host(session, drain_timeout)
            .await
    }

    /// Load one installed package from Control selection + Artifact Store.
    ///
    /// This is the production read seam for Host/CLI after Control cutover.
    /// It never reads legacy `extensions/` receipts.
    pub async fn installed_extension(
        &self,
        package_id: &str,
    ) -> UseResult<Option<a3s_use_extension::InstalledExtension>> {
        let package_id = a3s_use_core::PluginPackageId::parse(package_id.to_string())?;
        let control = self.ensure_control().await?;
        let Some(snapshot) = control.current_snapshot().await? else {
            return Ok(None);
        };
        let Some(selection) = snapshot.package_selection(package_id.as_str()) else {
            return Ok(None);
        };
        Ok(Some(
            self.registry
                .load_control_package_selection(selection)
                .await?,
        ))
    }

    /// Read the exact installed dependency lock owned by one root package.
    ///
    /// Embedding hosts use this immutable evidence when creating reviewed
    /// upgrade and uninstall plans. The package graph store remains owned by
    /// A3S Use; callers cannot replace or remove records through this API.
    pub async fn installed_package_lock(
        &self,
        root_package_id: &str,
    ) -> UseResult<Option<PluginPackageLock>> {
        let control = self.ensure_control().await?;
        let Some(snapshot) = control.current_snapshot().await? else {
            return Ok(None);
        };
        snapshot.package_lock(root_package_id)
    }

    /// Snapshot every exact dependency lock currently owned by A3S Use.
    ///
    /// Results are sorted by root package ID by the durable graph store. This
    /// lets an embedding host retain shared dependencies during reviewed graph
    /// upgrades and removals without parsing Use-owned state files.
    pub async fn installed_package_locks(&self) -> UseResult<Vec<PluginPackageLock>> {
        let control = self.ensure_control().await?;
        let Some(snapshot) = control.current_snapshot().await? else {
            return Ok(Vec::new());
        };
        snapshot.package_locks()
    }

    fn download_attempt_store(&self) -> PackageDownloadAttemptStore {
        PackageDownloadAttemptStore::new(self.registry.paths().installation_state_root())
    }

    fn resolution_attempt_store(&self) -> PackageResolutionAttemptStore {
        PackageResolutionAttemptStore::new(self.registry.paths().installation_state_root())
    }

    fn maintenance_lock(&self) -> a3s_use_extension::StateMaintenanceLock {
        a3s_use_extension::StateMaintenanceLock::new(self.registry.paths().state_root())
    }

    fn installation_mutation_lock(&self) -> mutation_lock::InstallationMutationLock {
        mutation_lock::InstallationMutationLock::new(
            self.registry.paths().installation_state_root(),
        )
    }

    async fn require_graph_mutation_domain(
        &self,
        _action: PluginOperationAction,
        _root_package_id: &str,
    ) -> UseResult<()> {
        // Control single-writer gate is the Control operation aggregate.
        // Never probe package-enablement or operations/package-graphs.
        Ok(())
    }

    pub(crate) async fn current_capability_generation(&self) -> UseResult<u64> {
        let control = self.ensure_control().await?;
        Ok(control
            .current_snapshot()
            .await?
            .map(|snapshot| snapshot.generation)
            .unwrap_or(0))
    }

    /// Control-owned Registry face for operation diagnostics.
    ///
    /// Generation and digest bind the Control installation snapshot. Pending
    /// `registry.json` cutovers are always empty under Control; cutover status
    /// is projected from lifecycle/Grant observation and generation comparison.
    pub(crate) async fn control_registry_diagnostic_face(
        &self,
    ) -> UseResult<(
        u64,
        String,
        Vec<a3s_use_extension::ExtensionRegistryCutoverRecord>,
    )> {
        crate::control_store::reject_legacy_authority_paths(
            &self.registry.paths().installation_state_root(),
        )?;
        let control = self.ensure_control().await?;
        match control.current_snapshot().await? {
            Some(snapshot) => Ok((
                snapshot.generation,
                snapshot.descriptor_digest()?,
                Vec::new(),
            )),
            None => {
                let empty = a3s_use_extension::ExtensionRegistrySnapshot::empty(
                    self.registry.installation().clone(),
                )?;
                Ok((0, empty.descriptor_digest()?, Vec::new()))
            }
        }
    }

    /// Grant snapshot for planning without creating the legacy `grants/` leaf.
    ///
    /// Product hosts (CLI Plugin Manager) must use this instead of
    /// [`a3s_use_extension::WorkspaceGrantStore::from_extension_paths`], which
    /// materializes `grants/` and poison Control open via
    /// `reject_legacy_authority_paths`.
    pub async fn planned_grant_snapshot(
        &self,
        state_revision: u64,
    ) -> UseResult<a3s_use_core::PluginWorkspaceGrantSnapshot> {
        let control = self.ensure_control().await?;
        control
            .planned_grant_snapshot(&self.scope().id, state_revision)
            .await
    }

    /// Observe one Control-owned Workspace Grant without opening the legacy
    /// `grants/` leaf.
    pub async fn observe_stored_workspace_grant(
        &self,
        package_id: &str,
        package_digest: &str,
    ) -> UseResult<Option<a3s_use_extension::StoredWorkspaceGrant>> {
        let control = self.ensure_control().await?;
        control
            .observe_stored_workspace_grant(&self.scope().id, package_id, package_digest)
            .await
    }

    /// Admit without writing `operations/package-graphs` (Control sole authority).
    pub(super) async fn admit_planned_graph_operation_in_memory(
        &self,
        pending: PendingPackageGraphOperation,
    ) -> UseResult<PendingPackageGraphOperation> {
        match pending.phase() {
            PackageGraphOperationPhase::Cancelled => Err(package_manager_error(
                "use.plugin.package_graph_cancelled",
                "The reviewed cognitive-package operation was cancelled before admission.",
            )),
            PackageGraphOperationPhase::Admitted => {
                self.authorization.verify_plan(&pending.envelope)?;
                Ok(pending)
            }
            PackageGraphOperationPhase::Planned => {
                let actual_generation = self.current_capability_generation().await?;
                let expected_generation = pending.envelope.plan.state.capability_generation;
                if actual_generation != expected_generation {
                    return Err(package_manager_error(
                        "use.plugin.package_generation_changed",
                        "The reviewed package graph generation changed before admission.",
                    )
                    .with_detail("expectedCapabilityGeneration", expected_generation)
                    .with_detail("actualCapabilityGeneration", actual_generation));
                }
                let grant_snapshot = self
                    .planned_grant_snapshot(pending.envelope.plan.state.state_revision)
                    .await?;
                let grants = grant::reconstruct_planned_workspace_grants(
                    &pending.envelope.plan,
                    &grant_snapshot,
                )?;
                let admitted_at_ms = plan::now_ms()?;
                let authorization = grant::authorize_planned_operation(
                    self.authorization.as_ref(),
                    &pending.envelope,
                    grants.as_ref(),
                    admitted_at_ms,
                )
                .await?;
                pending.admit(admitted_at_ms, authorization)
            }
        }
    }
}

pub(super) fn current_host_target() -> UseResult<String> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => Ok("darwin-arm64".to_string()),
        ("macos", "x86_64") => Ok("darwin-x86_64".to_string()),
        ("linux", "aarch64") => Ok("linux-arm64".to_string()),
        ("linux", "x86_64") => Ok("linux-x86_64".to_string()),
        ("windows", "x86_64") => Ok("windows-x86_64".to_string()),
        (os, arch) => Err(package_manager_error(
            "use.plugin.package_host_unsupported",
            format!("Cognitive packages do not support host target '{os}-{arch}'."),
        )),
    }
}

pub(super) fn all_catalog_surfaces(
    package: &LockedPluginPackage,
) -> Vec<a3s_use_core::PluginSurfaceRef> {
    package
        .catalog
        .record
        .surfaces
        .iter()
        .map(a3s_use_core::CatalogSurface::reference)
        .collect()
}

pub(super) fn installed_matches_lock(
    installed: &InstalledExtension,
    catalog: &VerifiedPluginCatalogRecord,
) -> UseResult<bool> {
    if installed.receipt.lifecycle_generation.is_none() {
        return Ok(false);
    }
    Ok(installed.plan_ready_catalog()? == catalog)
}

pub(super) fn package_manager_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

fn deny_unsigned_control_when_signed_trust_required(
    opened_with_signed_trust: bool,
    require_signed: bool,
) -> UseResult<()> {
    if require_signed && !opened_with_signed_trust {
        return Err(package_manager_error(
            "use.control.signed_description_trust_unavailable",
            "Control was already opened without Registry/TUF description trust; restart the host and open Control through ensure_control_for_registry before serving the Capability Gateway.",
        ));
    }
    Ok(())
}

pub(super) fn installation_mutation_busy(
    action: &str,
    package_id: &str,
    operation_id: &str,
) -> UseError {
    package_manager_error(
        "use.plugin.package_graph_busy",
        format!(
            "Admitted '{action}' operation for cognitive package '{package_id}' owns the installation mutation domain."
        ),
    )
    .with_detail("activeOperationId", operation_id.to_string())
    .with_detail("activePackageId", package_id.to_string())
}

pub(super) fn plugin_operation_action_name(action: PluginOperationAction) -> &'static str {
    match action {
        PluginOperationAction::Install => "install",
        PluginOperationAction::Upgrade => "upgrade",
        PluginOperationAction::Uninstall => "uninstall",
        PluginOperationAction::Enable => "enable",
        PluginOperationAction::Disable => "disable",
    }
}

#[cfg(test)]
mod control_open_tests {
    use super::deny_unsigned_control_when_signed_trust_required;

    #[test]
    fn unsigned_open_may_continue_when_signed_trust_not_required() {
        deny_unsigned_control_when_signed_trust_required(false, false).unwrap();
        deny_unsigned_control_when_signed_trust_required(true, false).unwrap();
        deny_unsigned_control_when_signed_trust_required(true, true).unwrap();
    }

    #[test]
    fn unsigned_open_fails_closed_when_signed_trust_required() {
        let error = deny_unsigned_control_when_signed_trust_required(false, true).unwrap_err();
        assert_eq!(error.code, "use.control.signed_description_trust_unavailable");
    }
}
