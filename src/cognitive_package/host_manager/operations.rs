//! Internal graph/apply/observe operations for CognitivePackageHostManager.

use super::*;
use super::helpers::*;

impl CognitivePackageHostManager {

    pub(super) async fn plan_graph(
        &self,
        request: &PluginHostPlanRequest,
        access: CognitiveRegistryAccess,
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
                            .prepare_install_with_access_selected(
                                sources.root(),
                                sources.dependencies(),
                                request.package_id.as_str(),
                                requested_version,
                                candidate.record.channel,
                                &lock_digest,
                                registry_access(access),
                                &request.selected_surfaces,
                            )
                            .await
                    }
                    PluginOperationAction::Upgrade => {
                        self.manager
                            .prepare_upgrade_with_access_selected(
                                sources.root(),
                                sources.dependencies(),
                                request.package_id.as_str(),
                                requested_version,
                                candidate.record.channel,
                                &lock_digest,
                                registry_access(access),
                                &request.selected_surfaces,
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

    pub(super) async fn resolve_sources(
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

    pub(super) fn reviewed_manager(
        &self,
        envelope: PluginOperationPlanEnvelope,
        confirmation: Option<a3s_use_core::PluginOperationConfirmation>,
    ) -> UseResult<CognitivePackageManager> {
        let authorization =
            ReviewedCognitivePackageAuthorizationProvider::new(envelope, confirmation)?;
        CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
            self.manager.registry.clone(),
            self.manager.scope().clone(),
            self.manager.lifecycle.clone(),
            Arc::new(authorization),
        )
    }

    pub(super) async fn apply_graph(
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
                        .install_cached_selected(
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
                        .upgrade_cached_selected(
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

    pub(super) async fn apply_enablement(
        &self,
        request: &PluginHostApplyRequest,
        stored_request: &PluginHostEnablementPlanRequest,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<AppliedOutcome> {
        let cognitive_request = cognitive_enablement_request(stored_request, envelope)?;
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

    pub(super) async fn graph_completion(
        &self,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<Option<u64>> {
        // Control is sole mutable authority: prefer Control observation.
        let control = self.manager.ensure_control().await?;
        if let Some(observed) = control
            .observe_operation(&envelope.plan.operation_id)
            .await?
        {
            if !observed.matches_envelope(envelope) {
                return Err(host_error(
                    "use.plugin.host_operation_observation_mismatch",
                    "Control graph completion differs from the Host plan envelope.",
                ));
            }
            if observed.phase == crate::control_store::ControlObservedOperationPhase::Completed {
                return Ok(observed.completed_at_ms);
            }
            return Ok(None);
        }
        Ok(None)
    }

    pub(super) fn verify_live_apply(
        &self,
        request: &PluginHostApplyRequest,
        plan: &StoredPluginHostPlan,
        current_time_ms: u64,
    ) -> UseResult<()> {
        match plan {
            StoredPluginHostPlan::Graph { result, .. } => {
                request.verify_apply_for_plan(result, &self.capabilities, current_time_ms)
            }
            StoredPluginHostPlan::Enablement { result, .. } => request
                .verify_apply_for_enablement_plan(result, &self.capabilities, current_time_ms),
        }
    }

    pub(super) fn verify_admitted_replay(
        &self,
        request: &PluginHostApplyRequest,
        plan: &StoredPluginHostPlan,
    ) -> UseResult<()> {
        match plan {
            StoredPluginHostPlan::Graph { result, .. } => {
                request.verify_admitted_replay_for_plan(result, &self.capabilities)
            }
            StoredPluginHostPlan::Enablement { result, .. } => {
                request.verify_admitted_replay_for_enablement_plan(result, &self.capabilities)
            }
        }
    }

    pub(super) async fn has_durable_admission(&self, plan: &StoredPluginHostPlan) -> UseResult<bool> {
        match plan {
            StoredPluginHostPlan::Graph { result, .. } => {
                if self.graph_completion(&result.plan).await?.is_some() {
                    return Ok(true);
                }
                let control = self.manager.ensure_control().await?;
                let Some(observed) = control
                    .observe_operation(&result.plan.plan.operation_id)
                    .await?
                else {
                    return Ok(false);
                };
                if !observed.matches_envelope(&result.plan) {
                    return Err(host_error(
                        "use.plugin.host_admission_mismatch",
                        "Control package graph evidence does not match the stored Host plan.",
                    ));
                }
                Ok(matches!(
                    observed.phase,
                    crate::control_store::ControlObservedOperationPhase::InFlight
                        | crate::control_store::ControlObservedOperationPhase::Completed
                ))
            }
            StoredPluginHostPlan::Enablement { request, result } => {
                let Some(envelope) = result.plan.as_ref() else {
                    return Ok(false);
                };
                let cognitive_request = cognitive_enablement_request(request, envelope)?;
                let control = self.manager.ensure_control().await?;
                let Some(observed) = control
                    .observe_operation(&cognitive_request.operation_id)
                    .await?
                else {
                    return Ok(false);
                };
                if !observed.matches_envelope(envelope) {
                    return Err(host_error(
                        "use.plugin.host_admission_mismatch",
                        "Control enablement evidence does not match the stored Host plan.",
                    ));
                }
                Ok(matches!(
                    observed.phase,
                    crate::control_store::ControlObservedOperationPhase::InFlight
                        | crate::control_store::ControlObservedOperationPhase::Completed
                ))
            }
        }
    }

    pub(super) async fn operation_status(
        &self,
        stored: &StoredPluginHostRequest,
    ) -> UseResult<PluginHostOperationStatus> {
        let envelope = stored.plan.envelope().ok_or_else(|| {
            host_error(
                "use.plugin.host_enablement_no_change",
                "A no-change Host plan has no operation to observe.",
            )
        })?;
        if let Some(cancellation) = self
            .store
            .get_cancellation(
                stored.plan.scope(),
                &envelope.plan.operation_id,
                &envelope.plan_digest,
            )
            .await?
        {
            if cancellation.plan_digest != envelope.plan_digest {
                return Err(host_error(
                    "use.plugin.host_cancellation_mismatch",
                    "The durable cancellation does not bind the observed plan.",
                ));
            }
            return Ok(PluginHostOperationStatus {
                phase: PluginHostOperationPhase::Cancelled,
                cancellability: PluginHostOperationCancellability::NotApplicable,
                progress: None,
                error_code: None,
                completed_at_ms: Some(cancellation.cancelled_at_ms),
                operation_result_digest: None,
                state: None,
            });
        }
        if let StoredPluginHostPlan::Graph { result, .. } = &stored.plan {
            let control = self.manager.ensure_control().await?;
            if let Some(observed) = control
                .observe_operation(&result.plan.plan.operation_id)
                .await?
            {
                if !observed.matches_envelope(&result.plan) {
                    return Err(host_error(
                        "use.plugin.host_operation_observation_mismatch",
                        "The Control graph operation differs from the observed Host plan.",
                    ));
                }
                if observed.phase == crate::control_store::ControlObservedOperationPhase::Cancelled
                {
                    return Ok(PluginHostOperationStatus {
                        phase: PluginHostOperationPhase::Cancelled,
                        cancellability: PluginHostOperationCancellability::NotApplicable,
                        progress: None,
                        error_code: None,
                        completed_at_ms: observed.completed_at_ms,
                        operation_result_digest: None,
                        state: None,
                    });
                }
            }
        }
        if let Some(outcome) = &stored.outcome {
            return Ok(PluginHostOperationStatus {
                phase: PluginHostOperationPhase::Completed,
                cancellability: PluginHostOperationCancellability::NotApplicable,
                progress: None,
                error_code: None,
                completed_at_ms: Some(outcome.completed_at_ms),
                operation_result_digest: Some(outcome.operation_result_digest.clone()),
                state: Some(outcome.state.clone()),
            });
        }
        if envelope.plan.authority.decision == PlanPolicyDecision::Deny {
            return Ok(PluginHostOperationStatus {
                phase: PluginHostOperationPhase::Denied,
                cancellability: PluginHostOperationCancellability::NotApplicable,
                progress: None,
                error_code: None,
                completed_at_ms: Some(envelope.plan.created_at_ms),
                operation_result_digest: None,
                state: None,
            });
        }

        let awaiting_confirmation = envelope.plan.authority.decision == PlanPolicyDecision::Ask;
        let phase = if awaiting_confirmation {
            PluginHostOperationPhase::AwaitingConfirmation
        } else {
            PluginHostOperationPhase::Planned
        };
        let cancellability = PluginHostOperationCancellability::Cancellable;
        let progress = None;
        match &stored.plan {
            StoredPluginHostPlan::Graph { result, .. } => {
                let control = self.manager.ensure_control().await?;
                if let Some(observed) = control
                    .observe_operation(&result.plan.plan.operation_id)
                    .await?
                {
                    if !observed.matches_envelope(&result.plan) {
                        return Err(host_error(
                            "use.plugin.host_operation_observation_mismatch",
                            "The Control graph operation differs from the observed Host plan.",
                        ));
                    }
                    if observed.phase
                        == crate::control_store::ControlObservedOperationPhase::Completed
                    {
                        return Ok(PluginHostOperationStatus {
                            phase: PluginHostOperationPhase::Finalizing,
                            cancellability: PluginHostOperationCancellability::TooLate,
                            progress: None,
                            error_code: None,
                            completed_at_ms: observed.completed_at_ms,
                            operation_result_digest: observed.result_digest.clone(),
                            state: None,
                        });
                    }
                    if observed.phase
                        == crate::control_store::ControlObservedOperationPhase::InFlight
                    {
                        return Ok(PluginHostOperationStatus {
                            phase: PluginHostOperationPhase::Preparing,
                            cancellability: PluginHostOperationCancellability::TooLate,
                            progress: None,
                            error_code: None,
                            completed_at_ms: None,
                            operation_result_digest: None,
                            state: None,
                        });
                    }
                    if observed.phase
                        == crate::control_store::ControlObservedOperationPhase::Rejected
                    {
                        return Ok(PluginHostOperationStatus {
                            phase: PluginHostOperationPhase::Failed,
                            cancellability: PluginHostOperationCancellability::NotApplicable,
                            progress: None,
                            error_code: Some("use.control_store.operation_rejected".to_string()),
                            completed_at_ms: observed.completed_at_ms,
                            operation_result_digest: None,
                            state: None,
                        });
                    }
                }
            }
            StoredPluginHostPlan::Enablement { request, result } => {
                if let Some(status) = self
                    .enablement_operation_status(request, result, envelope)
                    .await?
                {
                    return Ok(status);
                }
            }
        }

        Ok(PluginHostOperationStatus {
            phase,
            cancellability,
            progress,
            error_code: None,
            completed_at_ms: None,
            operation_result_digest: None,
            state: None,
        })
    }

    pub(super) async fn enablement_operation_status(
        &self,
        request: &PluginHostEnablementPlanRequest,
        _result: &PluginHostEnablementPlanResult,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<Option<PluginHostOperationStatus>> {
        let cognitive_request = cognitive_enablement_request(request, envelope)?;
        self.enablement_operation_status_control(&cognitive_request, envelope)
            .await
    }

    pub(super) async fn enablement_operation_status_control(
        &self,
        cognitive_request: &super::super::enablement::CognitivePackageEnablementRequest,
        envelope: &PluginOperationPlanEnvelope,
    ) -> UseResult<Option<PluginHostOperationStatus>> {
        let control = self.manager.ensure_control().await?;
        let Some(observed) = control
            .observe_operation(&cognitive_request.operation_id)
            .await?
        else {
            return Ok(None);
        };
        if !observed.matches_envelope(envelope) {
            return Err(host_operation_observation_mismatch(
                "Control enablement evidence differs from the observed Host plan.",
            ));
        }
        Ok(Some(match observed.phase {
            crate::control_store::ControlObservedOperationPhase::InFlight => {
                PluginHostOperationStatus {
                    phase: PluginHostOperationPhase::Preparing,
                    cancellability: PluginHostOperationCancellability::TooLate,
                    progress: None,
                    error_code: None,
                    completed_at_ms: None,
                    operation_result_digest: None,
                    state: None,
                }
            }
            crate::control_store::ControlObservedOperationPhase::Completed => {
                PluginHostOperationStatus {
                    phase: PluginHostOperationPhase::Finalizing,
                    cancellability: PluginHostOperationCancellability::TooLate,
                    progress: None,
                    error_code: None,
                    completed_at_ms: observed.completed_at_ms,
                    operation_result_digest: observed.result_digest.clone(),
                    state: None,
                }
            }
            crate::control_store::ControlObservedOperationPhase::Cancelled => {
                PluginHostOperationStatus {
                    phase: PluginHostOperationPhase::Cancelled,
                    cancellability: PluginHostOperationCancellability::NotApplicable,
                    progress: None,
                    error_code: None,
                    completed_at_ms: observed.completed_at_ms,
                    operation_result_digest: None,
                    state: None,
                }
            }
            crate::control_store::ControlObservedOperationPhase::Rejected => {
                PluginHostOperationStatus {
                    phase: PluginHostOperationPhase::Failed,
                    cancellability: PluginHostOperationCancellability::NotApplicable,
                    progress: None,
                    error_code: Some("use.control_store.operation_rejected".to_string()),
                    completed_at_ms: observed.completed_at_ms,
                    operation_result_digest: None,
                    state: None,
                }
            }
        }))
    }

    pub(super) async fn observe_operation_once(
        &self,
        request: &PluginHostOperationObservationRequest,
    ) -> UseResult<PluginHostOperationObservationResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        self.verify_fence(&request.scope)?;
        let stored = self
            .store
            .get_by_operation(&request.scope, &request.operation_id, &request.plan_digest)
            .await?
            .ok_or_else(|| {
                host_error(
                    "use.plugin.host_plan_missing",
                    "The observed operation has no durable Host plan record.",
                )
            })?;
        let envelope = stored.plan.envelope().ok_or_else(|| {
            host_error(
                "use.plugin.host_enablement_no_change",
                "A no-change Host plan has no operation to observe.",
            )
        })?;
        if stored.plan.scope() != &request.scope
            || envelope.plan.package_id != request.package_id.as_str()
            || envelope.plan.operation_id != request.operation_id
            || envelope.plan_digest != request.plan_digest
        {
            return Err(host_error(
                "use.plugin.host_operation_observation_mismatch",
                "The operation observation does not bind the exact stored plan.",
            ));
        }
        let status = self.operation_status(&stored).await?;
        let result = PluginHostOperationObservationResult {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_RESULT_SCHEMA.to_owned(),
            request_id: request.request_id.clone(),
            assignment_generation: request.assignment_generation,
            capabilities_digest: request.capabilities_digest.clone(),
            scope: request.scope.clone(),
            package_id: request.package_id.clone(),
            operation_id: request.operation_id.clone(),
            plan_digest: request.plan_digest.clone(),
            observed_at_ms: now_ms()?,
            revision: status.descriptor_digest()?,
            changed: true,
            timed_out: false,
            status,
        };
        result.validate_for(request, &self.capabilities)?;
        Ok(result)
    }

    pub(super) async fn outcome_is_current(&self, stored: &StoredPluginHostRequest) -> UseResult<bool> {
        let Some(outcome) = stored.outcome.as_ref() else {
            return Ok(true);
        };
        let envelope = stored.plan.envelope().ok_or_else(|| {
            host_error(
                "use.plugin.host_enablement_no_change",
                "A no-change Host plan cannot retain an operation outcome.",
            )
        })?;
        if let StoredPluginHostPlan::Graph { .. } = &stored.plan {
            if self.graph_completion(envelope).await? != Some(outcome.completed_at_ms) {
                return Ok(false);
            }
            let installed = self
                .manager
                .installed_package_lock(&envelope.plan.package_id)
                .await?;
            match envelope.plan.action {
                PluginOperationAction::Install | PluginOperationAction::Upgrade => {
                    if installed.as_ref() != envelope.package_lock.as_ref() {
                        return Ok(false);
                    }
                }
                PluginOperationAction::Uninstall => {
                    if installed.is_some() {
                        return Ok(false);
                    }
                }
                PluginOperationAction::Enable | PluginOperationAction::Disable => {
                    return Err(host_error(
                        "use.plugin.host_operation_result_mismatch",
                        "A graph Host outcome contains an enablement action.",
                    ));
                }
            }
        }
        let current = self
            .manager
            .observe_package(&envelope.plan.package_id)
            .await?;
        Ok(package_outcome_matches(&current, &outcome.state))
    }

    pub(crate) async fn plan_cognitive_package(
        &self,
        request: PluginHostPlanRequest,
        access: CognitiveRegistryAccess,
    ) -> UseResult<PluginHostPlanResult> {
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
            if !self.outcome_is_current(&record).await? {
                return Err(host_outcome_stale());
            }
            let mut replay = stored_result.clone();
            replay.replayed = true;
            replay.validate_for(&request, &self.capabilities)?;
            return Ok(replay);
        }

        let envelope = self.plan_graph(&request, access).await?;
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

    pub(super) fn verify_fence(&self, scope: &PluginManagedScope) -> UseResult<()> {
        scope.verify_current_fence(&self.current_scope)
    }
}

