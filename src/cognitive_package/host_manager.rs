use std::sync::Arc;

use a3s_use_core::{
    PlanPackageRole, PluginDesiredState, PluginHostApplyRequest, PluginHostApplyResult,
    PluginHostCapabilities, PluginHostEnablementPlanRequest, PluginHostEnablementPlanResult,
    PluginHostEnablementPlanStatus, PluginHostManager, PluginHostObservationRequest,
    PluginHostObservationResult, PluginHostObservationStatus, PluginHostPackageState,
    PluginHostPlanRequest, PluginHostPlanResult, PluginManagedScope, PluginObservedState,
    PluginOperationAction, PluginOperationPlanEnvelope, PluginPackageLock, PluginSurfaceRef,
    UseError, UseResult, VerifiedPluginCatalogRecord, PLUGIN_HOST_APPLY_RESULT_SCHEMA,
    PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA, PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA,
    PLUGIN_HOST_PLAN_RESULT_SCHEMA,
};
use a3s_use_extension::{
    ExtensionRegistry, RegistrySourceStore, ResolvedRegistrySources, TrustedRegistry,
};
use async_trait::async_trait;
use serde::Serialize;

use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleJournalStore, PluginLifecycleOperationStatus,
};

use super::enablement_plan::CognitivePackageEnablementPlanStatus;
use super::host_store::{
    digest_value, PluginHostProtocolStore, StoredPluginHostOutcome, StoredPluginHostPlan,
    StoredPluginHostRequest,
};
use super::plan::now_ms;
use super::{
    CognitivePackageAuthorizationProvider, CognitivePackageEnablementRequest,
    CognitivePackageLifecycleFactory, CognitivePackageManager,
    ReviewedCognitivePackageAuthorizationProvider, COGNITIVE_PACKAGE_HOST_VERSION,
};

const HOST_OPERATION_OUTCOME_SCHEMA: &str = "a3s.use.plugin-host-operation-outcome.v1";

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
    /// Compose one manager for an exact durable workspace fence.
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
        let capabilities = PluginHostCapabilities::v4(
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
            registry_sources: RegistrySourceStore::new(paths.clone()),
            manager,
            store: PluginHostProtocolStore::new(paths.state_root()),
        })
    }

    pub fn managed_scope(&self) -> &PluginManagedScope {
        &self.current_scope
    }

    pub fn host_capabilities(&self) -> &PluginHostCapabilities {
        &self.capabilities
    }

    async fn plan_graph(
        &self,
        request: &PluginHostPlanRequest,
    ) -> UseResult<PluginOperationPlanEnvelope> {
        let lock = require_request_lock(request)?;
        let lock_digest = lock.descriptor_digest()?;
        match request.action {
            PluginOperationAction::Install | PluginOperationAction::Upgrade => {
                let candidate = request.candidate.as_ref().ok_or_else(|| {
                    host_error(
                        "use.plugin.host_catalog_required",
                        "A managed install or upgrade requires an exact verified catalog candidate.",
                    )
                })?;
                let sources = self.resolve_sources(candidate).await?;
                let requested_version = Some(candidate.record.version.as_str());
                match request.action {
                    PluginOperationAction::Install => {
                        self.manager
                            .prepare_install_remote_selected(
                                sources.root(),
                                sources.dependencies(),
                                request.package_id.as_str(),
                                requested_version,
                                candidate.record.channel,
                                &request.selected_surfaces,
                                &lock_digest,
                            )
                            .await
                    }
                    PluginOperationAction::Upgrade => {
                        self.manager
                            .prepare_upgrade_remote_selected(
                                sources.root(),
                                sources.dependencies(),
                                request.package_id.as_str(),
                                requested_version,
                                candidate.record.channel,
                                &request.selected_surfaces,
                                &lock_digest,
                            )
                            .await
                    }
                    PluginOperationAction::Uninstall
                    | PluginOperationAction::Enable
                    | PluginOperationAction::Disable => Err(host_error(
                        "use.plugin.host_plan_action_unsupported",
                        "The managed graph planner received an unsupported action.",
                    )),
                }
            }
            PluginOperationAction::Uninstall => {
                self.manager
                    .prepare_uninstall(request.package_id.as_str(), &lock_digest)
                    .await
            }
            PluginOperationAction::Enable | PluginOperationAction::Disable => Err(host_error(
                "use.plugin.host_plan_action_unsupported",
                "Enablement must use the reviewed Plugin Host enablement port.",
            )),
        }
    }

    async fn resolve_sources(
        &self,
        candidate: &VerifiedPluginCatalogRecord,
    ) -> UseResult<ResolvedRegistrySources> {
        let provenance = &candidate.provenance;
        let sources = self
            .registry_sources
            .resolve(Some(&provenance.registry_name))
            .await?;
        verify_registry_provenance(sources.root(), candidate)?;
        Ok(sources)
    }

    fn reviewed_manager(
        &self,
        envelope: PluginOperationPlanEnvelope,
        confirmation: Option<a3s_use_core::PluginOperationConfirmation>,
    ) -> UseResult<CognitivePackageManager> {
        let authorization =
            ReviewedCognitivePackageAuthorizationProvider::new(envelope, confirmation)?;
        CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
            self.manager.registry.clone(),
            self.manager.scope.clone(),
            self.manager.lifecycle.clone(),
            Arc::new(authorization),
        )
    }

    async fn apply_graph(
        &self,
        request: &PluginHostApplyRequest,
        stored_request: &PluginHostPlanRequest,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<AppliedOutcome> {
        let completion_before = self.graph_completion(envelope).await?;
        let reviewed = self.reviewed_manager(envelope.clone(), request.confirmation.clone())?;
        let underlying_replayed = match envelope.plan.action {
            PluginOperationAction::Install | PluginOperationAction::Upgrade => {
                let lock = require_request_lock(stored_request)?;
                let lock_digest = lock.descriptor_digest()?;
                let candidate = stored_request.candidate.as_ref().ok_or_else(|| {
                    host_error(
                        "use.plugin.host_catalog_required",
                        "The stored managed operation omitted its verified catalog candidate.",
                    )
                })?;
                let sources = self.resolve_sources(candidate).await?;
                let result = match envelope.plan.action {
                    PluginOperationAction::Install => reviewed
                        .install_remote_selected(
                            sources.root(),
                            sources.dependencies(),
                            stored_request.package_id.as_str(),
                            Some(candidate.record.version.as_str()),
                            candidate.record.channel,
                            &stored_request.selected_surfaces,
                            Some(&lock_digest),
                        )
                        .await
                        .map(|result| (result.changed, result.plan)),
                    PluginOperationAction::Upgrade => reviewed
                        .upgrade_remote_selected(
                            sources.root(),
                            sources.dependencies(),
                            stored_request.package_id.as_str(),
                            Some(candidate.record.version.as_str()),
                            candidate.record.channel,
                            &stored_request.selected_surfaces,
                            Some(&lock_digest),
                        )
                        .await
                        .map(|result| (result.changed, result.plan)),
                    PluginOperationAction::Uninstall
                    | PluginOperationAction::Enable
                    | PluginOperationAction::Disable => Err(host_error(
                        "use.plugin.host_plan_action_unsupported",
                        "The managed graph apply path received an unsupported action.",
                    )),
                }?;
                if result.0 && result.1.as_ref() != Some(envelope) {
                    return Err(host_error(
                        "use.plugin.host_operation_result_mismatch",
                        "The package manager applied a different reviewed graph plan.",
                    ));
                }
                !result.0
            }
            PluginOperationAction::Uninstall => {
                match reviewed.uninstall(stored_request.package_id.as_str()).await {
                    Ok(result) => {
                        if result.plan != *envelope {
                            return Err(host_error(
                                "use.plugin.host_operation_result_mismatch",
                                "The package manager applied a different reviewed uninstall plan.",
                            ));
                        }
                        false
                    }
                    Err(error)
                        if error.code == "use.plugin.package_graph_missing"
                            && completion_before.is_some() =>
                    {
                        true
                    }
                    Err(error) => return Err(error),
                }
            }
            PluginOperationAction::Enable | PluginOperationAction::Disable => {
                return Err(host_error(
                    "use.plugin.host_plan_action_unsupported",
                    "A graph Host operation cannot apply enablement.",
                ))
            }
        };

        let completed_at_ms = self.graph_completion(envelope).await?.ok_or_else(|| {
            host_error(
                "use.plugin.host_operation_evidence_missing",
                "The completed package graph has no exact durable lifecycle operation evidence.",
            )
        })?;
        let state = self
            .manager
            .observe_package(stored_request.package_id.as_str())
            .await?;
        validate_state_for_plan(&state, envelope)?;
        let operation_result_digest = graph_outcome_digest(envelope, completed_at_ms, &state)?;
        Ok(AppliedOutcome {
            completed_at_ms,
            operation_result_digest,
            state,
            replayed: underlying_replayed,
        })
    }

    async fn apply_enablement(
        &self,
        request: &PluginHostApplyRequest,
        stored_request: &PluginHostEnablementPlanRequest,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<AppliedOutcome> {
        let cognitive_request = CognitivePackageEnablementRequest::new(
            envelope.plan.operation_id.clone(),
            stored_request.package_id.to_string(),
            stored_request.expected_package_generation,
            stored_request.enabled,
        )?;
        let result = self
            .manager
            .apply_enablement(
                &cognitive_request,
                envelope.clone(),
                request.confirmation.clone(),
            )
            .await?;
        validate_state_for_plan(&result.state, envelope)?;
        Ok(AppliedOutcome {
            completed_at_ms: result.completed_at_ms,
            operation_result_digest: result.operation_result_digest,
            state: result.state,
            replayed: result.replayed,
        })
    }

    async fn graph_completion(
        &self,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<Option<u64>> {
        let store =
            PluginLifecycleJournalStore::from_extension_paths(self.manager.registry.paths());
        for record in [
            store
                .load_active(&self.manager.scope, &envelope.plan.package_id)
                .await?,
            store
                .load_last(&self.manager.scope, &envelope.plan.package_id)
                .await?,
        ]
        .into_iter()
        .flatten()
        {
            if record.status == PluginLifecycleOperationStatus::Completed
                && record.intent.operation_id == envelope.plan.operation_id
                && record.intent.plan_digest == envelope.plan_digest
                && record.intent.scope == self.manager.scope
                && record.intent.package_id == envelope.plan.package_id
                && record.intent.action == lifecycle_action(envelope.plan.action)?
            {
                return Ok(record.completed_at_ms);
            }
        }
        Ok(None)
    }

    fn verify_fence(&self, scope: &PluginManagedScope) -> UseResult<()> {
        scope.verify_current_fence(&self.current_scope)
    }
}

#[async_trait]
impl PluginHostManager for CognitivePackageHostManager {
    async fn capabilities(&self) -> UseResult<PluginHostCapabilities> {
        self.capabilities.validate()?;
        Ok(self.capabilities.clone())
    }

    async fn plan(&self, request: PluginHostPlanRequest) -> UseResult<PluginHostPlanResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        self.verify_fence(&request.scope)?;
        require_request_lock(&request)?;
        let _request_lock = self
            .store
            .lock_request(&request.scope, &request.request_id)
            .await?;
        if let Some(record) = self
            .store
            .get_by_request(&request.scope, &request.request_id)
            .await?
        {
            let Some((stored_request, stored_result)) = record.plan.graph_parts() else {
                return Err(host_store_conflict());
            };
            if stored_request != &request {
                return Err(host_store_conflict());
            }
            let mut replay = stored_result.clone();
            replay.replayed = true;
            replay.validate_for(&request, &self.capabilities)?;
            return Ok(replay);
        }

        let envelope = self.plan_graph(&request).await?;
        let result = PluginHostPlanResult {
            schema: PLUGIN_HOST_PLAN_RESULT_SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            assignment_generation: request.assignment_generation,
            capabilities_digest: request.capabilities_digest.clone(),
            scope: request.scope.clone(),
            package_id: request.package_id.clone(),
            plan: envelope,
            replayed: false,
        };
        result.validate_for(&request, &self.capabilities)?;
        let stored = StoredPluginHostRequest::new(StoredPluginHostPlan::graph(
            request.clone(),
            result.clone(),
        )?)?;
        let inserted = self.store.put_plan(&stored).await?;
        let mut result = result;
        result.replayed = !inserted;
        result.validate_for(&request, &self.capabilities)?;
        Ok(result)
    }

    async fn apply(&self, request: PluginHostApplyRequest) -> UseResult<PluginHostApplyResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        self.verify_fence(&request.scope)?;
        let _operation_lock = self
            .store
            .lock_operation(&request.scope, &request.operation_id)
            .await?;
        let stored = self
            .store
            .get_by_operation(&request.scope, &request.operation_id)
            .await?
            .ok_or_else(|| {
                host_error(
                    "use.plugin.host_plan_missing",
                    "The digest-only apply request has no durable Host plan record.",
                )
            })?;

        match &stored.plan {
            StoredPluginHostPlan::Graph { result, .. } => {
                request.verify_apply_for_plan(result, &self.capabilities, now_ms()?)?;
            }
            StoredPluginHostPlan::Enablement { result, .. } => {
                request.verify_apply_for_enablement_plan(result, &self.capabilities, now_ms()?)?;
            }
        }
        if let Some(outcome) = &stored.outcome {
            return apply_result(&request, outcome, true, &self.capabilities);
        }

        let envelope = stored.plan.envelope().ok_or_else(|| {
            host_error(
                "use.plugin.host_enablement_no_change",
                "A no-change Host plan has no operation to apply.",
            )
        })?;
        let applied = match &stored.plan {
            StoredPluginHostPlan::Graph {
                request: stored_request,
                ..
            } => self.apply_graph(&request, stored_request, envelope).await?,
            StoredPluginHostPlan::Enablement {
                request: stored_request,
                ..
            } => {
                self.apply_enablement(&request, stored_request, envelope)
                    .await?
            }
        };
        let outcome = StoredPluginHostOutcome::new(
            applied.completed_at_ms,
            applied.operation_result_digest,
            applied.state,
        )?;
        let (_, inserted) = self.store.put_outcome(&stored, outcome.clone()).await?;
        apply_result(
            &request,
            &outcome,
            applied.replayed || !inserted,
            &self.capabilities,
        )
    }

    async fn plan_enablement(
        &self,
        request: PluginHostEnablementPlanRequest,
    ) -> UseResult<PluginHostEnablementPlanResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        self.verify_fence(&request.scope)?;
        let _request_lock = self
            .store
            .lock_request(&request.scope, &request.request_id)
            .await?;
        if let Some(record) = self
            .store
            .get_by_request(&request.scope, &request.request_id)
            .await?
        {
            let Some((stored_request, stored_result)) = record.plan.enablement_parts() else {
                return Err(host_store_conflict());
            };
            if stored_request != &request {
                return Err(host_store_conflict());
            }
            let mut replay = stored_result.clone();
            replay.replayed = true;
            replay.validate_for(&request, &self.capabilities)?;
            return Ok(replay);
        }

        let operation_id = enablement_operation_id(&request)?;
        let cognitive_request = CognitivePackageEnablementRequest::new(
            operation_id,
            request.package_id.to_string(),
            request.expected_package_generation,
            request.enabled,
        )?;
        let planned = self.manager.plan_enablement(&cognitive_request).await?;
        let (status, plan) = match planned.status {
            CognitivePackageEnablementPlanStatus::NoChange => {
                (PluginHostEnablementPlanStatus::NoChange, None)
            }
            CognitivePackageEnablementPlanStatus::Planned => (
                PluginHostEnablementPlanStatus::Planned,
                Some(planned.plan.clone().ok_or_else(|| {
                    host_error(
                        "use.plugin.host_enablement_plan_invalid",
                        "The cognitive-package planner omitted its immutable enablement plan.",
                    )
                })?),
            ),
            CognitivePackageEnablementPlanStatus::Completed => {
                return Err(host_error(
                    "use.plugin.host_enablement_already_completed",
                    "The deterministic enablement operation already completed without its Host request record; observe the current generation before replanning.",
                ))
            }
        };
        let result = PluginHostEnablementPlanResult {
            schema: PLUGIN_HOST_ENABLEMENT_PLAN_RESULT_SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            assignment_generation: request.assignment_generation,
            capabilities_digest: request.capabilities_digest.clone(),
            scope: request.scope.clone(),
            package_id: request.package_id.clone(),
            expected_package_generation: request.expected_package_generation,
            enabled: request.enabled,
            planned_at_ms: planned.planned_at_ms,
            status,
            state: planned.state,
            plan,
            replayed: false,
        };
        result.validate_for(&request, &self.capabilities)?;
        let stored = StoredPluginHostRequest::new(StoredPluginHostPlan::enablement(
            request.clone(),
            result.clone(),
        )?)?;
        let inserted = self.store.put_plan(&stored).await?;
        let mut result = result;
        result.replayed = !inserted;
        result.validate_for(&request, &self.capabilities)?;
        Ok(result)
    }

    async fn observe(
        &self,
        request: PluginHostObservationRequest,
    ) -> UseResult<PluginHostObservationResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        self.verify_fence(&request.scope)?;
        let state = self
            .manager
            .observe_package(request.package_id.as_str())
            .await?;
        let result = PluginHostObservationResult {
            schema: PLUGIN_HOST_OBSERVATION_RESULT_SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            assignment_generation: request.assignment_generation,
            capabilities_digest: request.capabilities_digest.clone(),
            scope: request.scope.clone(),
            package_id: request.package_id.clone(),
            observed_at_ms: now_ms()?,
            status: PluginHostObservationStatus::Available { state },
        };
        result.validate_for(&request, &self.capabilities)?;
        Ok(result)
    }
}

struct AppliedOutcome {
    completed_at_ms: u64,
    operation_result_digest: String,
    state: PluginHostPackageState,
    replayed: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct GraphOperationOutcome<'a> {
    schema: &'a str,
    operation_id: &'a str,
    plan_digest: &'a str,
    completed_at_ms: u64,
    state: &'a PluginHostPackageState,
}

fn graph_outcome_digest(
    envelope: &PluginOperationPlanEnvelope,
    completed_at_ms: u64,
    state: &PluginHostPackageState,
) -> UseResult<String> {
    digest_value(&GraphOperationOutcome {
        schema: HOST_OPERATION_OUTCOME_SCHEMA,
        operation_id: &envelope.plan.operation_id,
        plan_digest: &envelope.plan_digest,
        completed_at_ms,
        state,
    })
}

fn apply_result(
    request: &PluginHostApplyRequest,
    outcome: &StoredPluginHostOutcome,
    replayed: bool,
    capabilities: &PluginHostCapabilities,
) -> UseResult<PluginHostApplyResult> {
    let result = PluginHostApplyResult {
        schema: PLUGIN_HOST_APPLY_RESULT_SCHEMA.to_string(),
        request_id: request.request_id.clone(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: request.operation_id.clone(),
        plan_digest: request.plan_digest.clone(),
        completed_at_ms: outcome.completed_at_ms,
        operation_result_digest: outcome.operation_result_digest.clone(),
        state: outcome.state.clone(),
        replayed,
    };
    result.validate_for(request, capabilities)?;
    Ok(result)
}

fn require_request_lock(request: &PluginHostPlanRequest) -> UseResult<&PluginPackageLock> {
    request.package_lock.as_ref().ok_or_else(|| {
        host_error(
            "use.plugin.host_package_lock_required",
            "The cognitive-package Host adapter requires the exact resolved package lock for every graph operation.",
        )
    })
}

fn verify_registry_provenance(
    registry: &TrustedRegistry,
    candidate: &VerifiedPluginCatalogRecord,
) -> UseResult<()> {
    let provenance = &candidate.provenance;
    if !registry.matches_provenance(provenance) {
        return Err(host_error(
            "use.plugin.host_registry_provenance_mismatch",
            "The configured Registry source no longer matches the reviewed catalog provenance.",
        ));
    }
    Ok(())
}

fn validate_state_for_plan(
    state: &PluginHostPackageState,
    envelope: &PluginOperationPlanEnvelope,
) -> UseResult<()> {
    state.validate()?;
    envelope.validate()?;
    let root = envelope
        .plan
        .packages
        .iter()
        .find(|package| package.role == PlanPackageRole::Root)
        .ok_or_else(|| {
            host_error(
                "use.plugin.host_operation_result_mismatch",
                "The reviewed plan omitted its root package transition.",
            )
        })?;
    if envelope.plan.action == PluginOperationAction::Uninstall {
        if state.desired == PluginDesiredState::Absent
            && state.observed == PluginObservedState::Removed
            && state.version.is_none()
            && state.selected_surfaces.is_empty()
        {
            return Ok(());
        }
        return Err(host_error(
            "use.plugin.host_operation_result_mismatch",
            "The uninstall outcome did not remove the reviewed root package.",
        ));
    }
    let expected = root.after.as_ref().ok_or_else(|| {
        host_error(
            "use.plugin.host_operation_result_mismatch",
            "The reviewed operation has no expected root package state.",
        )
    })?;
    let selected_surfaces = expected
        .release
        .surfaces
        .iter()
        .map(a3s_use_core::CatalogSurface::reference)
        .collect::<Vec<PluginSurfaceRef>>();
    let desired = match envelope.plan.action {
        PluginOperationAction::Install | PluginOperationAction::Upgrade => {
            PluginDesiredState::Enabled
        }
        PluginOperationAction::Enable => PluginDesiredState::Enabled,
        PluginOperationAction::Disable => PluginDesiredState::InstalledDisabled,
        PluginOperationAction::Uninstall => {
            return Err(host_error(
                "use.plugin.host_operation_result_mismatch",
                "The uninstall outcome retained an unexpected root package state.",
            ))
        }
    };
    if state.version.as_deref() != Some(expected.release.version.as_str())
        || state.package_digest.as_deref() != Some(expected.release.package_sha256.as_str())
        || state.manifest_digest.as_deref() != Some(expected.release.manifest_sha256.as_str())
        || state.selected_surfaces != selected_surfaces
        || state.desired != desired
    {
        return Err(host_error(
            "use.plugin.host_operation_result_mismatch",
            "The observed package state does not match the exact reviewed operation plan.",
        ));
    }
    Ok(())
}

fn lifecycle_action(action: PluginOperationAction) -> UseResult<PluginLifecycleAction> {
    match action {
        PluginOperationAction::Install => Ok(PluginLifecycleAction::Install),
        PluginOperationAction::Upgrade => Ok(PluginLifecycleAction::Upgrade),
        PluginOperationAction::Uninstall => Ok(PluginLifecycleAction::Uninstall),
        PluginOperationAction::Enable | PluginOperationAction::Disable => Err(host_error(
            "use.plugin.host_plan_action_unsupported",
            "Enablement does not use package graph completion evidence.",
        )),
    }
}

fn enablement_operation_id(request: &PluginHostEnablementPlanRequest) -> UseResult<String> {
    let digest = request.descriptor_digest()?;
    let digest = digest.strip_prefix("sha256:").ok_or_else(|| {
        host_error(
            "use.plugin.host_enablement_plan_invalid",
            "The enablement request digest has an invalid encoding.",
        )
    })?;
    let action = if request.enabled { "enable" } else { "disable" };
    Ok(format!("{action}:host:{}", &digest[..32]))
}

fn host_store_conflict() -> UseError {
    host_error(
        "use.plugin.host_store_conflict",
        "The Host request ID already owns a different operation kind or request body.",
    )
}

fn host_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
