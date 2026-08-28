//! Production composition for schema-v3 cognitive-package graphs.
//!
//! Registry endpoints and trust roots are supplied by the embedding host.
//! This module owns dependency-lock planning and package-level lifecycle
//! composition; Tool, MCP, Skill, UI, and OKF remain contributions inside one
//! immutable package generation.

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
mod host_store;
mod hosts;
mod install;
mod mutation_lock;
mod native_provider;
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

use crate::plugin_lifecycle::PluginLifecycleCoordinator;
use download_attempt::PackageDownloadAttemptStore;
use resolution_attempt::PackageResolutionAttemptStore;
use store::{
    InstalledPackageGraphStore, PackageGraphOperationPhase, PendingPackageGraphOperation,
    PendingPackageGraphStore,
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
pub use grant::{
    bind_cognitive_package_grant_impacts, bind_cognitive_package_grants,
    reconstruct_cognitive_package_grants, CognitivePackageAuthorizationEvidence,
    CognitivePackageAuthorizationProvider, CognitivePackageGrantPlan,
    StandaloneCognitivePackageAuthorizationProvider,
};
pub use host_manager::CognitivePackageHostManager;
pub use hosts::{
    ManagedCognitivePackageLifecycleFactory, StandaloneCognitivePackageLifecycleFactory,
    A3S_FLOW_NATIVE_TS_COMPILER_ENV,
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

/// Host composition boundary for one package-owned lifecycle saga.
///
/// A3S Code, A3S OS, or another embedding host injects exact Runtime, Gateway,
/// Knowledge, Skill, and UI adapters here. The standalone implementation is
/// deliberately narrower and fails closed for surfaces whose owner is absent.
pub trait CognitivePackageLifecycleFactory: Send + Sync {
    fn name(&self) -> &'static str;

    fn validate_manifest(&self, manifest: &ExtensionManifest) -> UseResult<()>;

    /// Validate host-owned lifecycle availability before Runtime provider
    /// preflight. Managed hosts override this so a provider-neutral enablement
    /// draft can be created before its exact Runtime selection exists.
    fn validate_manifest_for_planning(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        self.validate_manifest(manifest)
    }

    /// Validate adapters needed only to retire an already receipt-bound
    /// generation. Runtime provider ownership is resolved from durable binding
    /// receipts, so managed hosts must not require a new activation selection.
    fn validate_manifest_for_retirement(&self, manifest: &ExtensionManifest) -> UseResult<()> {
        self.validate_manifest(manifest)
    }

    fn install_coordinator(
        &self,
        registry: ExtensionRegistry,
        candidate: ExtensionLifecyclePackage,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator>;

    fn published_install_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator>;

    fn uninstall_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator>;

    /// Compose enable/disable over an already committed immutable generation.
    ///
    /// Enablement has no package commit or removal checkpoint, but it must use
    /// the exact same Runtime, Gateway, Knowledge, Flow, Skill, UI, and
    /// capability hosts as install and uninstall.
    fn enablement_coordinator(
        &self,
        registry: ExtensionRegistry,
        package_root: PathBuf,
    ) -> UseResult<PluginLifecycleCoordinator> {
        self.published_install_coordinator(registry, package_root)
    }
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
        Ok(Self {
            registry,
            lifecycle,
            authorization,
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

    /// Read the exact installed dependency lock owned by one root package.
    ///
    /// Embedding hosts use this immutable evidence when creating reviewed
    /// upgrade and uninstall plans. The package graph store remains owned by
    /// A3S Use; callers cannot replace or remove records through this API.
    pub async fn installed_package_lock(
        &self,
        root_package_id: &str,
    ) -> UseResult<Option<PluginPackageLock>> {
        Ok(self
            .graph_store()
            .get(root_package_id)
            .await?
            .map(|graph| graph.package_lock))
    }

    /// Snapshot every exact dependency lock currently owned by A3S Use.
    ///
    /// Results are sorted by root package ID by the durable graph store. This
    /// lets an embedding host retain shared dependencies during reviewed graph
    /// upgrades and removals without parsing Use-owned state files.
    pub async fn installed_package_locks(&self) -> UseResult<Vec<PluginPackageLock>> {
        Ok(self
            .graph_store()
            .list()
            .await?
            .into_iter()
            .map(|graph| graph.package_lock)
            .collect())
    }

    fn graph_store(&self) -> InstalledPackageGraphStore {
        InstalledPackageGraphStore::new(self.registry.paths().installation_state_root())
    }

    fn pending_store(&self) -> PendingPackageGraphStore {
        PendingPackageGraphStore::new(self.registry.paths().installation_state_root())
    }

    fn download_attempt_store(&self) -> PackageDownloadAttemptStore {
        PackageDownloadAttemptStore::new(self.registry.paths().installation_state_root())
    }

    fn resolution_attempt_store(&self) -> PackageResolutionAttemptStore {
        PackageResolutionAttemptStore::new(self.registry.paths().installation_state_root())
    }

    fn grant_store(&self) -> a3s_use_extension::WorkspaceGrantStore {
        a3s_use_extension::WorkspaceGrantStore::from_extension_paths(self.registry.paths())
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
        action: PluginOperationAction,
        root_package_id: &str,
    ) -> UseResult<()> {
        if let Some(active) = self.enablement_store().active_operation().await? {
            let active_action = if active.request.enabled {
                "enable"
            } else {
                "disable"
            };
            return Err(installation_mutation_busy(
                active_action,
                active.request.package_id.as_str(),
                &active.request.operation_id,
            ));
        }
        if let Some(active) = self.pending_store().admitted_operation().await? {
            if active.action() != action || active.root_package_id() != root_package_id {
                return Err(installation_mutation_busy(
                    plugin_operation_action_name(active.action()),
                    active.root_package_id(),
                    &active.envelope.plan.operation_id,
                ));
            }
        }
        Ok(())
    }

    async fn admit_planned_graph_operation(
        &self,
        store: &PendingPackageGraphStore,
        pending: PendingPackageGraphOperation,
    ) -> UseResult<PendingPackageGraphOperation> {
        if let Some(active) = self.enablement_store().active_operation().await? {
            let action = if active.request.enabled {
                "enable"
            } else {
                "disable"
            };
            return Err(installation_mutation_busy(
                action,
                active.request.package_id.as_str(),
                &active.request.operation_id,
            ));
        }
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
                let snapshot = self.registry.snapshot().await?;
                let expected_generation = pending.envelope.plan.state.capability_generation;
                if snapshot.generation != expected_generation {
                    return Err(package_manager_error(
                        "use.plugin.package_generation_changed",
                        "The reviewed package graph generation changed before admission.",
                    )
                    .with_detail("expectedCapabilityGeneration", expected_generation)
                    .with_detail("actualCapabilityGeneration", snapshot.generation));
                }
                store.require_admission_available(&pending).await?;
                let grant_snapshot = self
                    .grant_store()
                    .snapshot_scope(&self.scope().id, pending.envelope.plan.state.state_revision)
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
                store
                    .admit(&pending, admitted_at_ms, authorization)
                    .await
                    .map(|(admitted, _)| admitted)
            }
        }
    }

    fn enablement_store(&self) -> enablement_store::CognitivePackageEnablementStore {
        enablement_store::CognitivePackageEnablementStore::new(
            self.registry.paths().installation_state_root(),
        )
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
