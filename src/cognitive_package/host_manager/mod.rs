use std::sync::Arc;

use a3s_use_core::{
    PlanPackageRole, PlanPolicyDecision, PluginDesiredState, PluginHostApplyRequest,
    PluginHostApplyResult, PluginHostCancelRequest, PluginHostCancelResult,
    PluginHostCancellationStatus, PluginHostCapabilities, PluginHostEnablementPlanRequest,
    PluginHostEnablementPlanResult, PluginHostEnablementPlanStatus, PluginHostManager,
    PluginHostObservationRequest, PluginHostObservationResult, PluginHostObservationStatus,
    PluginHostOperationCancellability, PluginHostOperationObservationRequest,
    PluginHostOperationObservationResult, PluginHostOperationPhase, PluginHostOperationStatus,
    PluginHostOperationWatchRequest, PluginHostPackageState, PluginHostPlanRequest,
    PluginHostPlanResult, PluginManagedScope, PluginObservedState, PluginOperationAction,
    PluginOperationPlanEnvelope, PluginPackageLock, PluginSurfaceRef, UseError, UseResult,
    VerifiedPluginCatalogRecord, PLUGIN_HOST_APPLY_RESULT_SCHEMA, PLUGIN_HOST_CANCEL_RESULT_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA, PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA,
    PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA, PLUGIN_HOST_PLAN_RESULT_SCHEMA,
};
use a3s_use_extension::{
    ExtensionRegistry, RegistrySourceStore, ResolvedRegistrySources, TrustedRegistry,
};
use async_trait::async_trait;
use serde::Serialize;

use super::embedded::{acquire_capability_lease, inspect_catalog, resolve_lock, search_catalogs};
use super::enablement_plan::CognitivePackageEnablementPlanStatus;
use super::host_store::{
    digest_value, PluginHostProtocolStore, StoredPluginHostCancellation, StoredPluginHostOutcome,
    StoredPluginHostPlan, StoredPluginHostRequest,
};
use super::plan::now_ms;
use super::registry_access::{resolve_package_lock, RegistryAccess};
use super::resolution_attempt::PendingPackageResolutionAttempt;
use super::{
    CognitiveCapabilityLease, CognitiveCatalogSearchResult, CognitivePackageAuthorizationProvider,
    CognitivePackageEnablementRequest, CognitivePackageLifecycleFactory, CognitivePackageManager,
    CognitiveRegistryAccess, ReviewedCognitivePackageAuthorizationProvider,
    COGNITIVE_PACKAGE_HOST_VERSION,
};

pub(super) const HOST_OPERATION_OUTCOME_SCHEMA: &str = "a3s.use.plugin-host-operation-outcome.v1";

/// Production adapter from the frozen Plugin Host protocol to the shared
/// cognitive-package manager.
///
/// This type owns no package lifecycle state machine. Plans, dependency locks,
/// grants, lifecycle checkpoints, Registry receipts, and observations remain
/// owned by [`CognitivePackageManager`]. Its durable store only binds remote
/// request IDs and digest-only apply calls to those exact Use-owned plans and
/// results.
#[derive(Clone)]
pub struct CognitivePackageHostManager {
    current_scope: PluginManagedScope,
    capabilities: PluginHostCapabilities,
    registry_sources: RegistrySourceStore,
    manager: CognitivePackageManager,
    store: PluginHostProtocolStore,
}

impl std::fmt::Debug for CognitivePackageHostManager {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CognitivePackageHostManager")
            .field("current_scope", &self.current_scope)
            .field("capabilities", &self.capabilities)
            .field("manager", &self.manager)
            .finish_non_exhaustive()
    }
}

impl CognitivePackageHostManager {
    /// Compose one manager for an exact durable User or Workspace fence.
    ///
    /// The embedding host supplies its lifecycle adapters and policy/provider
    /// authority. Registry source resolution is reused directly from the same
    /// [`ExtensionRegistry`] paths, so there is no second trust configuration.
    pub fn new(
        current_scope: PluginManagedScope,
        manager_build_id: impl Into<String>,
        registry: ExtensionRegistry,
        lifecycle: Arc<dyn CognitivePackageLifecycleFactory>,
        authorization: Arc<dyn CognitivePackageAuthorizationProvider>,
    ) -> UseResult<Self> {
        current_scope.validate()?;
        let capabilities = PluginHostCapabilities::v6(
            current_scope.host_id.clone(),
            COGNITIVE_PACKAGE_HOST_VERSION,
            manager_build_id,
        )?;
        let paths = registry.paths().clone();
        let manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
            registry,
            current_scope.plan_scope(),
            lifecycle,
            authorization,
        )?;
        Ok(Self {
            current_scope,
            capabilities,
            registry_sources: RegistrySourceStore::new(paths.use_paths().clone()),
            manager,
            store: PluginHostProtocolStore::new(paths.installation_state_root()),
        })
    }

    pub fn managed_scope(&self) -> &PluginManagedScope {
        &self.current_scope
    }

    pub fn host_capabilities(&self) -> &PluginHostCapabilities {
        &self.capabilities
    }

    /// Search all enabled, trusted Registries through the manager's embedded
    /// read-only boundary. No package mutation or model authority is involved.
    pub async fn search_cognitive_packages(
        &self,
        access: CognitiveRegistryAccess,
        selected_registry: Option<&str>,
        search: &a3s_use_extension::PluginCatalogSearch,
    ) -> UseResult<CognitiveCatalogSearchResult> {
        search_catalogs(&self.registry_sources, access, selected_registry, search).await
    }

    /// Reinspect the exact selected release and reject Registry provenance or
    /// snapshot drift before planning.
    pub async fn inspect_cognitive_package(
        &self,
        access: CognitiveRegistryAccess,
        candidate: &VerifiedPluginCatalogRecord,
    ) -> UseResult<a3s_use_extension::PluginCatalogInspection> {
        inspect_catalog(&self.registry_sources, access, candidate).await
    }

    /// Resolve an immutable complete dependency lock for one exact inspected
    /// release. The selected root must remain byte-for-byte identical.
    pub async fn resolve_cognitive_package_lock(
        &self,
        access: CognitiveRegistryAccess,
        candidate: &VerifiedPluginCatalogRecord,
    ) -> UseResult<PluginPackageLock> {
        resolve_lock(&self.registry_sources, access, candidate).await
    }

    /// Resolve one complete lock from a bounded SemVer selector through the
    /// host's configured Registry set. The returned root catalog record is the
    /// exact candidate that every presentation adapter must display.
    #[allow(clippy::too_many_arguments)]
    pub async fn resolve_cognitive_package_requirement(
        &self,
        action: PluginOperationAction,
        access: CognitiveRegistryAccess,
        selected_registry: Option<&str>,
        package_id: &str,
        version_requirement: Option<&str>,
        channel: a3s_use_core::PluginReleaseChannel,
        expected_package_lock_digest: Option<&str>,
    ) -> UseResult<PluginPackageLock> {
        if !matches!(
            action,
            PluginOperationAction::Install | PluginOperationAction::Upgrade
        ) {
            return Err(host_error(
                "use.plugin.host_plan_action_unsupported",
                "Only install and upgrade operations resolve Registry requirements.",
            ));
        }
        let sources = self.registry_sources.resolve(selected_registry).await?;
        let access = registry_access(access);
        let _maintenance = self.manager.maintenance_lock().acquire_shared().await?;
        let attempt = self
            .manager
            .resolution_attempt_store()
            .begin(PendingPackageResolutionAttempt::new(
                self.manager.scope().clone(),
                action,
                package_id,
                version_requirement,
                channel,
                access.resolution_access(),
                sources.root(),
                sources.dependencies(),
                now_ms()?,
            )?)
            .await?;
        let lock = match resolve_package_lock(
            access,
            sources.root(),
            sources.dependencies(),
            package_id,
            version_requirement,
            channel,
            &attempt,
        )
        .await
        {
            Ok(lock) => {
                attempt.mark_resolved(&lock).await?;
                lock
            }
            Err(error) => {
                attempt.mark_failed(&error.code).await?;
                return Err(error);
            }
        };
        let lock_digest = lock.descriptor_digest()?;
        super::install::verify_expected_lock(&lock_digest, expected_package_lock_digest)?;
        attempt.finish().await?;
        Ok(lock)
    }

    /// Load one installed package from Control selection + Artifact Store.
    pub async fn installed_cognitive_package(
        &self,
        package_id: &str,
    ) -> UseResult<Option<a3s_use_extension::InstalledExtension>> {
        self.manager.installed_extension(package_id).await
    }

    /// Enumerate selected package IDs without exposing receipt-owned paths.
    pub async fn installed_cognitive_package_ids(&self) -> UseResult<Vec<String>> {
        let snapshot = self
            .manager
            .ensure_control()
            .await?
            .current_snapshot()
            .await?;
        let Some(snapshot) = snapshot else {
            return Ok(Vec::new());
        };
        let mut package_ids = snapshot
            .packages
            .iter()
            .filter(|package| package.enabled)
            .map(|package| package.package_id().to_string())
            .collect::<Vec<_>>();
        package_ids.sort();
        package_ids.dedup();
        Ok(package_ids)
    }

    pub async fn installed_cognitive_package_lock(
        &self,
        package_id: &str,
    ) -> UseResult<Option<PluginPackageLock>> {
        self.manager.installed_package_lock(package_id).await
    }

    pub(crate) async fn cognitive_package_uninstall_plan_lock(
        &self,
        package_id: &str,
    ) -> UseResult<Option<PluginPackageLock>> {
        self.manager.uninstall_plan_lock(package_id).await
    }

    pub(crate) async fn current_package_graph_revision(&self) -> UseResult<(u64, String)> {
        let snapshot = self.manager.registry.snapshot().await?;
        Ok((snapshot.generation, snapshot.descriptor_digest()?))
    }

    /// Reopen the exact durable plan used by digest-only apply and UI review.
    pub async fn reviewed_cognitive_package_plan(
        &self,
        scope: &PluginManagedScope,
        operation_id: &str,
        plan_digest: &str,
    ) -> UseResult<PluginHostPlanResult> {
        self.verify_fence(scope)?;
        a3s_use_core::PluginOperationPlan::validate_operation_id(operation_id)?;
        let stored = self
            .store
            .get_by_operation(scope, operation_id, plan_digest)
            .await?
            .ok_or_else(|| {
                host_error(
                    "use.plugin.host_plan_missing",
                    "The reviewed operation has no durable Host plan record.",
                )
            })?;
        let result = match &stored.plan {
            StoredPluginHostPlan::Graph { result, .. } => result.as_ref().clone(),
            StoredPluginHostPlan::Enablement { result, .. } => result.reviewed_plan()?,
        };
        if result.plan.plan.operation_id != operation_id || result.plan.plan_digest != plan_digest {
            return Err(host_error(
                "use.plugin.host_plan_mismatch",
                "The reviewed operation does not bind the requested plan digest.",
            ));
        }
        result.validate()?;
        if result.scope != self.current_scope
            || result.capabilities_digest != self.capabilities.descriptor_digest()?
            || !self
                .capabilities
                .supports_plan_schema(&result.plan.plan.schema)
        {
            return Err(host_error(
                "use.plugin.host_plan_mismatch",
                "The reviewed operation does not match the current Host capabilities or scope.",
            ));
        }
        Ok(result)
    }

    /// Inspect presentation evidence signed by the same Registry snapshot as
    /// an exact catalog candidate. Missing presentation data is not an error.
    pub async fn inspect_cognitive_package_presentation(
        &self,
        access: CognitiveRegistryAccess,
        candidate: &VerifiedPluginCatalogRecord,
    ) -> UseResult<Option<a3s_use_extension::VerifiedCognitivePackagePresentation>> {
        candidate.validate()?;
        let sources = self.resolve_sources(candidate).await?;
        match access {
            CognitiveRegistryAccess::Refreshed => {
                a3s_use_extension::inspect_cognitive_package_presentation(sources.root(), candidate)
                    .await
            }
            CognitiveRegistryAccess::Cached => {
                a3s_use_extension::inspect_cached_cognitive_package_presentation(
                    sources.root(),
                    candidate,
                )
                .await
            }
        }
    }

    /// Fetch one exact presentation media target through the same verified
    /// Registry snapshot and bounded cache as its signed descriptor.
    ///
    /// The caller supplies no Registry URL, trust root, or arbitrary target
    /// path. The presentation evidence selects the configured Registry and the
    /// extension layer revalidates snapshot identity, media declaration,
    /// length, type, and digest before returning a local read-only file.
    pub async fn fetch_cognitive_package_media(
        &self,
        access: CognitiveRegistryAccess,
        presentation: &a3s_use_extension::VerifiedCognitivePackagePresentation,
        target_name: &str,
    ) -> UseResult<a3s_use_extension::VerifiedCognitivePackageMedia> {
        let sources = self
            .registry_sources
            .resolve(Some(&presentation.registry_name))
            .await?;
        match access {
            CognitiveRegistryAccess::Refreshed => {
                a3s_use_extension::fetch_cognitive_package_media(
                    sources.root(),
                    presentation,
                    target_name,
                )
                .await
            }
            CognitiveRegistryAccess::Cached => {
                a3s_use_extension::fetch_cached_cognitive_package_media(
                    sources.root(),
                    presentation,
                    target_name,
                )
                .await
            }
        }
    }

    /// Acquire one exact manager-scoped OKF capability after successful apply.
    /// `None` means the generation is no longer callable or is draining.
    pub async fn acquire_cognitive_capability(
        &self,
        scope: &PluginManagedScope,
        package_id: &str,
        surface_id: &str,
    ) -> UseResult<Option<CognitiveCapabilityLease>> {
        self.verify_fence(scope)?;
        acquire_capability_lease(&self.manager, &scope.plan_scope(), package_id, surface_id).await
    }
}

mod helpers;
mod operations;
mod plugin_host;
use helpers::*;
use operations::*;
use plugin_host::*;
