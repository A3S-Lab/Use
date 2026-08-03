use a3s_runtime::contract::{
    RuntimeActionRequest, RuntimeFeature, RuntimeHealthState, RuntimeInspection,
    RuntimeObservation, RuntimeRemoval, RuntimeUnitClass, RuntimeUnitState,
};
use a3s_use_core::{UseError, UseResult};
use serde::Serialize;

use super::client::{runtime_capabilities_digest, runtime_error, PluginRuntimeClient};
use super::model::{
    runtime_contract_error, RuntimeServiceBindingReceipt, RuntimeServiceReadinessEvidence,
};
use super::receipt::RuntimeBindingReceipt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RuntimeBindingObservedState {
    Prepared,
    Starting,
    Healthy,
    Failed,
    Draining,
    Stopped,
    Missing,
    Stale,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeBindingObservation {
    pub state: RuntimeBindingObservedState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation: Option<RuntimeObservation>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_generation: Option<u64>,
}

impl PluginRuntimeClient {
    pub async fn observe_binding(
        &self,
        receipt: &RuntimeBindingReceipt,
    ) -> UseResult<RuntimeBindingObservation> {
        receipt.validate()?;
        self.verify_receipt_provider(receipt, true).await?;
        match receipt {
            RuntimeBindingReceipt::Task(_) => Ok(RuntimeBindingObservation {
                state: RuntimeBindingObservedState::Prepared,
                observation: None,
                last_generation: None,
            }),
            RuntimeBindingReceipt::Service(receipt) => self.observe_service(receipt).await,
        }
    }

    pub async fn drain_remove_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        stop_request_id: impl Into<String>,
        remove_request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeRemoval> {
        self.stop_service(receipt, stop_request_id, deadline_at_ms)
            .await?;
        self.remove_service(receipt, remove_request_id, deadline_at_ms)
            .await
    }

    /// Idempotently stop one exact Runtime Service generation without removing
    /// its provider resource or durable binding receipt.
    pub async fn stop_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeInspection> {
        RuntimeBindingReceipt::Service(receipt.clone()).validate()?;
        self.verify_receipt_provider(&RuntimeBindingReceipt::Service(receipt.clone()), false)
            .await?;
        let inspection = self
            .client
            .inspect(&receipt.unit_id)
            .await
            .map_err(|error| runtime_error("inspect Runtime Service before drain", error))?;
        inspection.validate().map_err(runtime_contract_error)?;
        match inspection {
            RuntimeInspection::Found { observation, .. } => {
                validate_service_identity(receipt, &observation, false)?;
                if !observation.state.is_terminal() {
                    let stop = RuntimeActionRequest {
                        schema: RuntimeActionRequest::SCHEMA.to_string(),
                        request_id: request_id.into(),
                        unit_id: receipt.unit_id.clone(),
                        generation: receipt.generation,
                        deadline_at_ms,
                    };
                    stop.validate().map_err(runtime_contract_error)?;
                    let stopped = self
                        .client
                        .stop(&stop)
                        .await
                        .map_err(|error| runtime_error("drain Runtime Service", error))?;
                    stopped.validate().map_err(runtime_contract_error)?;
                    validate_stopped_inspection(receipt, &stopped)?;
                    return Ok(stopped);
                }
                Ok(RuntimeInspection::Found {
                    schema: RuntimeInspection::SCHEMA.to_string(),
                    observation,
                })
            }
            RuntimeInspection::NotFound {
                schema,
                unit_id,
                last_generation,
            } if unit_id == receipt.unit_id => Ok(RuntimeInspection::NotFound {
                schema,
                unit_id,
                last_generation,
            }),
            RuntimeInspection::NotFound { .. } => Err(runtime_contract_error(
                "Runtime inspection returned a different missing unit identity.",
            )),
        }
    }

    /// Idempotently remove one exact stopped Runtime Service generation.
    ///
    /// Callers keep the durable binding receipt until this returns matching
    /// provider evidence, so a crash can safely replay the same request.
    pub async fn remove_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
        request_id: impl Into<String>,
        deadline_at_ms: Option<u64>,
    ) -> UseResult<RuntimeRemoval> {
        RuntimeBindingReceipt::Service(receipt.clone()).validate()?;
        self.verify_receipt_provider(&RuntimeBindingReceipt::Service(receipt.clone()), false)
            .await?;
        let remove = RuntimeActionRequest {
            schema: RuntimeActionRequest::SCHEMA.to_string(),
            request_id: request_id.into(),
            unit_id: receipt.unit_id.clone(),
            generation: receipt.generation,
            deadline_at_ms,
        };
        remove.validate().map_err(runtime_contract_error)?;
        let removal = self
            .client
            .remove(&remove)
            .await
            .map_err(|error| runtime_error("remove Runtime Service", error))?;
        removal.validate().map_err(runtime_contract_error)?;
        if removal.request_id != remove.request_id
            || removal.unit_id != receipt.unit_id
            || removal.generation != receipt.generation
        {
            return Err(runtime_contract_error(
                "Runtime Service removal does not match the requested binding identity.",
            ));
        }
        Ok(removal)
    }

    async fn observe_service(
        &self,
        receipt: &RuntimeServiceBindingReceipt,
    ) -> UseResult<RuntimeBindingObservation> {
        let inspection = self
            .client
            .inspect(&receipt.unit_id)
            .await
            .map_err(|error| runtime_error("inspect Runtime Service binding", error))?;
        inspection.validate().map_err(runtime_contract_error)?;
        match inspection {
            RuntimeInspection::NotFound {
                unit_id,
                last_generation,
                ..
            } => {
                if unit_id != receipt.unit_id {
                    return Err(runtime_contract_error(
                        "Runtime inspection returned a different missing unit identity.",
                    ));
                }
                Ok(RuntimeBindingObservation {
                    state: RuntimeBindingObservedState::Missing,
                    observation: None,
                    last_generation,
                })
            }
            RuntimeInspection::Found { observation, .. } => {
                validate_service_identity(receipt, &observation, true)?;
                let state = observed_service_state(receipt, &observation);
                Ok(RuntimeBindingObservation {
                    state,
                    observation: Some(*observation),
                    last_generation: None,
                })
            }
        }
    }

    async fn verify_receipt_provider(
        &self,
        receipt: &RuntimeBindingReceipt,
        require_exact_evidence: bool,
    ) -> UseResult<()> {
        let capabilities = self
            .client
            .capabilities()
            .await
            .map_err(|error| runtime_error("read Runtime binding capabilities", error))?;
        capabilities.validate().map_err(runtime_contract_error)?;
        if capabilities.provider_id.as_str() != receipt.provider_id() {
            return Err(UseError::new(
                "use.plugin.runtime.provider_evidence_changed",
                "The injected Runtime client is not the binding's explicit provider.",
            ));
        }
        if require_exact_evidence
            && (capabilities.provider_build != receipt.provider_build_id()
                || runtime_capabilities_digest(&capabilities)? != receipt.capability_digest())
        {
            return Err(UseError::new(
                "use.plugin.runtime.provider_evidence_changed",
                "The Runtime binding provider evidence changed after preparation.",
            ));
        }
        let required = match receipt {
            RuntimeBindingReceipt::Task(_) => {
                vec![
                    RuntimeFeature::Logs,
                    RuntimeFeature::Stop,
                    RuntimeFeature::Remove,
                ]
            }
            RuntimeBindingReceipt::Service(_) => {
                vec![RuntimeFeature::Stop, RuntimeFeature::Remove]
            }
        };
        let missing = required
            .into_iter()
            .filter(|feature| !capabilities.supports_feature(*feature))
            .map(|feature| format!("feature:{feature:?}"))
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(UseError::new(
                "use.plugin.runtime.capability_missing",
                "The Runtime provider cannot safely drain and remove the binding.",
            )
            .with_detail("missing", serde_json::to_value(missing).unwrap_or_default()));
        }
        Ok(())
    }
}

fn validate_service_identity(
    receipt: &RuntimeServiceBindingReceipt,
    observation: &RuntimeObservation,
    require_active_evidence: bool,
) -> UseResult<()> {
    observation.validate().map_err(runtime_contract_error)?;
    if require_active_evidence
        && (observation.observed_at_ms == 0
            || observation
                .started_at_ms
                .is_some_and(|started_at_ms| started_at_ms > observation.observed_at_ms)
            || observation.health.as_ref().is_some_and(|health| {
                health.checked_at_ms == 0 || health.checked_at_ms > observation.observed_at_ms
            }))
    {
        return Err(runtime_contract_error(
            "The Runtime Service observation contains invalid timestamp evidence.",
        ));
    }
    if observation.unit_id != receipt.unit_id
        || observation.generation != receipt.generation
        || observation.spec_digest != receipt.spec_digest
        || observation.class != RuntimeUnitClass::Service
        || (require_active_evidence
            && observation.provider_build.as_deref() != Some(receipt.provider_build_id.as_str()))
    {
        return Err(UseError::new(
            "use.plugin.runtime.observation_evidence_mismatch",
            "The Runtime Service observation does not match its binding receipt.",
        ));
    }
    Ok(())
}

fn observed_service_state(
    receipt: &RuntimeServiceBindingReceipt,
    observation: &RuntimeObservation,
) -> RuntimeBindingObservedState {
    let health_regressed = observation.state == RuntimeUnitState::Running
        && observation
            .health
            .as_ref()
            .is_none_or(|health| health.checked_at_ms < receipt.last_healthy_at_ms);
    if observation.observed_at_ms < receipt.observation_revision
        || observation.started_at_ms != Some(receipt.runtime_started_at_ms)
        || health_regressed
        || matches!(
            &receipt.readiness,
            RuntimeServiceReadinessEvidence::McpInitialized { initialize }
                if observation
                    .started_at_ms
                    .is_some_and(|started| initialize.initialized_at_ms < started)
        )
    {
        return RuntimeBindingObservedState::Stale;
    }
    match observation.state {
        RuntimeUnitState::Accepted | RuntimeUnitState::Preparing | RuntimeUnitState::Starting => {
            RuntimeBindingObservedState::Starting
        }
        RuntimeUnitState::Running => match observation.health.as_ref().map(|health| health.state) {
            Some(RuntimeHealthState::Healthy) => RuntimeBindingObservedState::Healthy,
            Some(RuntimeHealthState::Unhealthy) => RuntimeBindingObservedState::Failed,
            Some(RuntimeHealthState::Unknown | RuntimeHealthState::Starting) | None => {
                RuntimeBindingObservedState::Starting
            }
        },
        RuntimeUnitState::Stopping => RuntimeBindingObservedState::Draining,
        RuntimeUnitState::Stopped => RuntimeBindingObservedState::Stopped,
        RuntimeUnitState::Failed | RuntimeUnitState::Unknown | RuntimeUnitState::Succeeded => {
            RuntimeBindingObservedState::Failed
        }
    }
}

fn validate_stopped_inspection(
    receipt: &RuntimeServiceBindingReceipt,
    inspection: &RuntimeInspection,
) -> UseResult<()> {
    match inspection {
        RuntimeInspection::NotFound { unit_id, .. } if unit_id == &receipt.unit_id => Ok(()),
        RuntimeInspection::Found { observation, .. } => {
            validate_service_identity(receipt, observation, false)?;
            if observation.state.is_terminal() {
                Ok(())
            } else {
                Err(UseError::new(
                    "use.plugin.runtime.drain_incomplete",
                    "The Runtime Service did not reach a terminal state before removal.",
                ))
            }
        }
        _ => Err(runtime_contract_error(
            "Runtime stop returned a different unit identity.",
        )),
    }
}
