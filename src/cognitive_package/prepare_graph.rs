use std::sync::{Arc, Mutex};

use a3s_use_core::{
    PlanAuthority, PluginOperationAction, PluginOperationPlan, PluginOperationPlanBinding,
    PluginOperationPlanDraft, PluginOperationPlanEnvelope, PluginReleaseChannel, PluginSurfaceRef,
    PluginWorkspaceGrantChangeSet, UseError, UseResult,
};
use a3s_use_extension::TrustedRegistry;
use async_trait::async_trait;

use super::grant::{CognitivePackageAuthorizationEvidence, CognitivePackageAuthorizationProvider};
use super::registry_access::RegistryAccess;
use super::{package_manager_error, CognitivePackageManager};

const PLAN_CAPTURED: &str = "use.plugin.package_graph_plan_captured";

#[derive(Clone)]
struct PlanningOnlyAuthorizationProvider {
    policy: Arc<dyn CognitivePackageAuthorizationProvider>,
    captured: Arc<Mutex<Option<PluginOperationPlanEnvelope>>>,
}

impl PlanningOnlyAuthorizationProvider {
    fn new(policy: Arc<dyn CognitivePackageAuthorizationProvider>) -> Self {
        Self {
            policy,
            captured: Arc::new(Mutex::new(None)),
        }
    }

    fn captured(&self) -> UseResult<PluginOperationPlanEnvelope> {
        self.captured
            .lock()
            .map_err(|_| plan_error("The package graph plan capture lock is unavailable."))?
            .clone()
            .ok_or_else(|| plan_error("The package graph planner stopped without a reviewed plan."))
    }
}

#[async_trait]
impl CognitivePackageAuthorizationProvider for PlanningOnlyAuthorizationProvider {
    fn name(&self) -> &'static str {
        "managed-plan-only"
    }

    fn reviewed_plan(&self) -> Option<&PluginOperationPlanEnvelope> {
        self.policy.reviewed_plan()
    }

    fn bind_authority(&self, draft: &PluginOperationPlanDraft) -> UseResult<PlanAuthority> {
        self.policy.bind_authority(draft)
    }

    fn bind_operation(
        &self,
        draft: &PluginOperationPlanDraft,
        default_binding: PluginOperationPlanBinding,
    ) -> UseResult<PluginOperationPlanBinding> {
        self.policy.bind_operation(draft, default_binding)
    }

    fn verify_authority(&self, plan: &PluginOperationPlan) -> UseResult<()> {
        self.policy.verify_authority(plan)
    }

    fn verify_plan(&self, envelope: &PluginOperationPlanEnvelope) -> UseResult<()> {
        self.policy.verify_plan(envelope)
    }

    async fn authorize(
        &self,
        envelope: &PluginOperationPlanEnvelope,
        _changes: Option<&PluginWorkspaceGrantChangeSet>,
        _now_ms: u64,
    ) -> UseResult<CognitivePackageAuthorizationEvidence> {
        self.policy.verify_plan(envelope)?;
        let mut captured = self
            .captured
            .lock()
            .map_err(|_| plan_error("The package graph plan capture lock is unavailable."))?;
        if captured.as_ref().is_some_and(|current| current != envelope) {
            return Err(plan_error(
                "The package graph planner produced more than one immutable plan.",
            ));
        }
        *captured = Some(envelope.clone());
        Err(UseError::new(
            PLAN_CAPTURED,
            "The immutable package graph plan is ready for host review.",
        ))
    }
}

impl CognitivePackageManager {
    /// Resolve, validate, and durably store an install plan without admitting
    /// or applying it. The exact lock digest is mandatory for managed-host
    /// planning, so a replay cannot silently select newer Registry metadata.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_install_remote(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: &str,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        self.prepare_install_remote_with_selection(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            None,
        )
        .await
    }

    /// Resolve and durably store a managed-host install plan for the exact
    /// mandatory closure plus the explicitly selected root surfaces.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_install_remote_selected(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        selected_surfaces: &[PluginSurfaceRef],
        expected_package_lock_digest: &str,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        self.prepare_install_remote_with_selection(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            Some(selected_surfaces),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn prepare_install_with_access_selected(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: &str,
        access: RegistryAccess,
        selected_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PluginOperationPlanEnvelope> {
        self.prepare_install_remote_with_selection(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            access,
            Some(selected_surfaces),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_install_remote_with_selection(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: &str,
        access: RegistryAccess,
        selected_surfaces: Option<&[PluginSurfaceRef]>,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        if selected_surfaces.is_none() {
            if let Some(plan) = self
                .existing_exact_graph_plan(
                    PluginOperationAction::Install,
                    package_id,
                    expected_package_lock_digest,
                )
                .await?
            {
                return Ok(plan);
            }
        }
        let planning = PlanningOnlyAuthorizationProvider::new(self.authorization.clone());
        let manager = self.with_planning_authorization(planning.clone())?;
        let result = manager
            .install_remote_with_access(
                root_registry,
                dependency_registries,
                package_id,
                requested_version,
                channel,
                Some(expected_package_lock_digest),
                access,
                selected_surfaces,
            )
            .await;
        match result {
            Err(error) if error.code == PLAN_CAPTURED => planning.captured(),
            Err(error) => Err(error),
            Ok(_) => Err(plan_error(
                "Install planning completed without producing a mutation plan.",
            )),
        }
    }

    /// Resolve, validate, and durably store an upgrade plan without admission
    /// or lifecycle mutation.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_upgrade_remote(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: &str,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        self.prepare_upgrade_remote_with_selection(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            None,
        )
        .await
    }

    /// Resolve and durably store a managed-host upgrade plan with an exact
    /// root surface selection.
    #[allow(clippy::too_many_arguments)]
    pub async fn prepare_upgrade_remote_selected(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        selected_surfaces: &[PluginSurfaceRef],
        expected_package_lock_digest: &str,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        self.prepare_upgrade_remote_with_selection(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            RegistryAccess::Refreshed,
            Some(selected_surfaces),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    pub(super) async fn prepare_upgrade_with_access_selected(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: &str,
        access: RegistryAccess,
        selected_surfaces: &[PluginSurfaceRef],
    ) -> UseResult<PluginOperationPlanEnvelope> {
        self.prepare_upgrade_remote_with_selection(
            root_registry,
            dependency_registries,
            package_id,
            requested_version,
            channel,
            expected_package_lock_digest,
            access,
            Some(selected_surfaces),
        )
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn prepare_upgrade_remote_with_selection(
        &self,
        root_registry: &TrustedRegistry,
        dependency_registries: &[TrustedRegistry],
        package_id: &str,
        requested_version: Option<&str>,
        channel: PluginReleaseChannel,
        expected_package_lock_digest: &str,
        access: RegistryAccess,
        selected_surfaces: Option<&[PluginSurfaceRef]>,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        if selected_surfaces.is_none() {
            if let Some(plan) = self
                .existing_exact_graph_plan(
                    PluginOperationAction::Upgrade,
                    package_id,
                    expected_package_lock_digest,
                )
                .await?
            {
                return Ok(plan);
            }
        }
        let planning = PlanningOnlyAuthorizationProvider::new(self.authorization.clone());
        let manager = self.with_planning_authorization(planning.clone())?;
        let result = manager
            .upgrade_remote_with_access(
                root_registry,
                dependency_registries,
                package_id,
                requested_version,
                channel,
                Some(expected_package_lock_digest),
                access,
                selected_surfaces,
            )
            .await;
        match result {
            Err(error) if error.code == PLAN_CAPTURED => planning.captured(),
            Err(error) => Err(error),
            Ok(_) => Err(plan_error(
                "Upgrade planning completed without producing a mutation plan.",
            )),
        }
    }

    /// Durably store an uninstall plan derived from the exact installed graph
    /// without admitting or applying it.
    pub async fn prepare_uninstall(
        &self,
        package_id: &str,
        expected_package_lock_digest: &str,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        if let Some(plan) = self
            .existing_exact_graph_plan(
                PluginOperationAction::Uninstall,
                package_id,
                expected_package_lock_digest,
            )
            .await?
        {
            return Ok(plan);
        }
        let installed = self
            .installed_package_lock(package_id)
            .await?
            .ok_or_else(|| {
                package_manager_error(
                    "use.plugin.package_graph_missing",
                    format!("Cognitive package '{package_id}' has no installed graph."),
                )
            })?;
        if installed.descriptor_digest()? != expected_package_lock_digest {
            return Err(plan_error(
                "The installed package graph does not match the requested lock digest.",
            ));
        }
        let planning = PlanningOnlyAuthorizationProvider::new(self.authorization.clone());
        let manager = self.with_planning_authorization(planning.clone())?;
        match manager.uninstall(package_id).await {
            Err(error) if error.code == PLAN_CAPTURED => planning.captured(),
            Err(error) => Err(error),
            Ok(_) => Err(plan_error(
                "Uninstall planning completed without producing a mutation plan.",
            )),
        }
    }

    fn with_planning_authorization(
        &self,
        authorization: PlanningOnlyAuthorizationProvider,
    ) -> UseResult<Self> {
        Self::with_plan_scope_lifecycle_and_authorization(
            self.registry.clone(),
            self.scope().clone(),
            self.lifecycle.clone(),
            Arc::new(authorization),
        )
    }

    async fn existing_exact_graph_plan(
        &self,
        action: PluginOperationAction,
        package_id: &str,
        expected_package_lock_digest: &str,
    ) -> UseResult<Option<PluginOperationPlanEnvelope>> {
        let Some(pending) = self.pending_store().get(action, package_id).await? else {
            return Ok(None);
        };
        if pending.phase() == super::store::PackageGraphOperationPhase::Cancelled {
            return Err(package_manager_error(
                "use.plugin.package_graph_cancelled",
                "The exact cognitive-package operation was cancelled before admission.",
            ));
        }
        let lock_digest = pending
            .envelope
            .package_lock
            .as_ref()
            .map(a3s_use_core::PluginPackageLock::descriptor_digest)
            .transpose()?
            .ok_or_else(|| plan_error("The stored package graph plan omitted its exact lock."))?;
        if lock_digest != expected_package_lock_digest {
            return Err(plan_error(
                "A different package graph plan is already pending for this package.",
            ));
        }
        self.authorization.verify_plan(&pending.envelope)?;
        Ok(Some(pending.envelope))
    }
}

fn plan_error(message: impl Into<String>) -> UseError {
    package_manager_error("use.plugin.package_graph_plan_invalid", message)
}
