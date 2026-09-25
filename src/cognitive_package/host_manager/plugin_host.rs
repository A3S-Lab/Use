//! PluginHostManager implementation for CognitivePackageHostManager.

use super::*;
use super::helpers::*;


#[async_trait]
impl PluginHostManager for CognitivePackageHostManager {
    async fn capabilities(&self) -> UseResult<PluginHostCapabilities> {
        self.capabilities.validate()?;
        Ok(self.capabilities.clone())
    }

    async fn plan(&self, request: PluginHostPlanRequest) -> UseResult<PluginHostPlanResult> {
        self.plan_cognitive_package(request, CognitiveRegistryAccess::Refreshed)
            .await
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
            .get_by_operation(&request.scope, &request.operation_id, &request.plan_digest)
            .await?
            .ok_or_else(|| {
                host_error(
                    "use.plugin.host_plan_missing",
                    "The digest-only apply request has no durable Host plan record.",
                )
            })?;

        let envelope = stored.plan.envelope().ok_or_else(|| {
            host_error(
                "use.plugin.host_enablement_no_change",
                "A no-change Host plan has no operation to apply.",
            )
        })?;
        if self
            .store
            .get_cancellation(&request.scope, &request.operation_id, &request.plan_digest)
            .await?
            .is_some()
        {
            return Err(host_error(
                "use.plugin.host_operation_cancelled",
                "The reviewed operation was cancelled before durable admission.",
            ));
        }
        if let Err(error) = self.verify_live_apply(&request, &stored.plan, now_ms()?) {
            if error.code != "use.plugin.plan_expired" {
                return Err(error);
            }
            let durably_admitted = if stored.outcome.is_some() {
                true
            } else {
                self.has_durable_admission(&stored.plan).await?
            };
            if !durably_admitted {
                return Err(error);
            }
            self.verify_admitted_replay(&request, &stored.plan)?;
        }
        if let Some(outcome) = &stored.outcome {
            if !self.outcome_is_current(&stored).await? {
                return Err(host_outcome_stale());
            }
            validate_state_for_plan(&outcome.state, envelope)?;
            return apply_result(&request, outcome, true, &self.capabilities);
        }

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

    async fn observe_operation(
        &self,
        request: PluginHostOperationObservationRequest,
    ) -> UseResult<PluginHostOperationObservationResult> {
        self.observe_operation_once(&request).await
    }

    async fn watch_operation(
        &self,
        request: PluginHostOperationWatchRequest,
    ) -> UseResult<PluginHostOperationObservationResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        let deadline = tokio::time::Instant::now()
            .checked_add(std::time::Duration::from_millis(request.timeout_ms))
            .ok_or_else(|| {
                host_error(
                    "use.plugin.host_operation_watch_timeout_invalid",
                    "The operation watch timeout is too large for this platform.",
                )
            })?;
        loop {
            let mut result = self.observe_operation_once(&request.observation).await?;
            if request.after_revision.as_deref() != Some(result.revision.as_str()) {
                result.changed = true;
                result.timed_out = false;
                return Ok(result);
            }
            let now = tokio::time::Instant::now();
            if request.timeout_ms == 0 || now >= deadline {
                result.changed = false;
                result.timed_out = true;
                result.validate_for(&request.observation, &self.capabilities)?;
                return Ok(result);
            }
            tokio::time::sleep(
                std::time::Duration::from_millis(50).min(deadline.saturating_duration_since(now)),
            )
            .await;
        }
    }

    async fn cancel(&self, request: PluginHostCancelRequest) -> UseResult<PluginHostCancelResult> {
        request.validate_for_capabilities(&self.capabilities)?;
        self.verify_fence(&request.scope)?;
        let _operation_lock = self
            .store
            .lock_operation(&request.scope, &request.operation_id)
            .await?;
        let stored = self
            .store
            .get_by_operation(&request.scope, &request.operation_id, &request.plan_digest)
            .await?
            .ok_or_else(|| {
                host_error(
                    "use.plugin.host_plan_missing",
                    "The cancelled operation has no durable Host plan record.",
                )
            })?;
        let envelope = stored.plan.envelope().ok_or_else(|| {
            host_error(
                "use.plugin.host_enablement_no_change",
                "A no-change Host plan has no operation to cancel.",
            )
        })?;
        if envelope.plan.package_id != request.package_id.as_str()
            || envelope.plan.operation_id != request.operation_id
            || envelope.plan_digest != request.plan_digest
        {
            return Err(host_error(
                "use.plugin.host_cancellation_mismatch",
                "The cancellation request does not bind the exact stored plan.",
            ));
        }
        let status = if let Some(cancellation) = self
            .store
            .get_cancellation(&request.scope, &request.operation_id, &request.plan_digest)
            .await?
        {
            if cancellation.plan_digest != request.plan_digest {
                return Err(host_error(
                    "use.plugin.host_cancellation_mismatch",
                    "The durable cancellation does not bind the exact requested plan.",
                ));
            }
            PluginHostCancellationStatus::AlreadyCancelled
        } else if stored.outcome.is_some() {
            PluginHostCancellationStatus::AlreadyCompleted
        } else {
            match &stored.plan {
                StoredPluginHostPlan::Graph { result, .. } => {
                    let control = self.manager.ensure_control().await?;
                    if let Some(observed) = control
                        .observe_operation(&result.plan.plan.operation_id)
                        .await?
                    {
                        if !observed.matches_envelope(&result.plan) {
                            return Err(host_error(
                                "use.plugin.host_cancellation_mismatch",
                                "Control package graph evidence differs from the cancelled Host plan.",
                            ));
                        }
                        match observed.phase {
                            crate::control_store::ControlObservedOperationPhase::InFlight
                            | crate::control_store::ControlObservedOperationPhase::Completed
                            | crate::control_store::ControlObservedOperationPhase::Rejected => {
                                PluginHostCancellationStatus::TooLate
                            }
                            crate::control_store::ControlObservedOperationPhase::Cancelled => {
                                let cancellation = StoredPluginHostCancellation::new(
                                    &request.request_id,
                                    &request.operation_id,
                                    &request.plan_digest,
                                    observed.completed_at_ms.unwrap_or(now_ms()?),
                                )?;
                                self.store
                                    .put_cancellation(&request.scope, &cancellation)
                                    .await?;
                                PluginHostCancellationStatus::AlreadyCancelled
                            }
                        }
                    } else if self
                        .manager
                        .has_retained_cancelled_graph(
                            result.package_id.as_str(),
                            &request.operation_id,
                            &request.plan_digest,
                        )
                        .await?
                    {
                        let cancellation = StoredPluginHostCancellation::new(
                            &request.request_id,
                            &request.operation_id,
                            &request.plan_digest,
                            now_ms()?,
                        )?;
                        self.store
                            .put_cancellation(&request.scope, &cancellation)
                            .await?;
                        PluginHostCancellationStatus::AlreadyCancelled
                    } else {
                        let cancelled_at_ms = now_ms()?;
                        let cancellation = StoredPluginHostCancellation::new(
                            &request.request_id,
                            &request.operation_id,
                            &request.plan_digest,
                            cancelled_at_ms,
                        )?;
                        self.store
                            .put_cancellation(&request.scope, &cancellation)
                            .await?;
                        self.manager
                            .retain_host_cancelled_graph_diagnostic(
                                &result.plan,
                                result.package_id.as_str(),
                                cancelled_at_ms,
                            )
                            .await?;
                        PluginHostCancellationStatus::Cancelled
                    }
                }
                StoredPluginHostPlan::Enablement { .. } => {
                    if self.has_durable_admission(&stored.plan).await? {
                        PluginHostCancellationStatus::TooLate
                    } else {
                        let cancellation = StoredPluginHostCancellation::new(
                            &request.request_id,
                            &request.operation_id,
                            &request.plan_digest,
                            now_ms()?,
                        )?;
                        self.store
                            .put_cancellation(&request.scope, &cancellation)
                            .await?;
                        PluginHostCancellationStatus::Cancelled
                    }
                }
            }
        };
        let result = PluginHostCancelResult {
            schema: PLUGIN_HOST_CANCEL_RESULT_SCHEMA.to_owned(),
            request_id: request.request_id.clone(),
            assignment_generation: request.assignment_generation,
            capabilities_digest: request.capabilities_digest.clone(),
            scope: request.scope.clone(),
            package_id: request.package_id.clone(),
            operation_id: request.operation_id.clone(),
            plan_digest: request.plan_digest.clone(),
            observed_at_ms: now_ms()?,
            status,
        };
        result.validate_for(&request, &self.capabilities)?;
        Ok(result)
    }
}
