use std::sync::Arc;

use a3s_use_core::{UseError, UseResult};
use sha2::{Digest, Sha256};

use super::effect_port::{
    ControlCapabilityCutoverRequest, ControlEffectFailure, ControlEffectPortOutcome,
    ControlEffectRequestIdentity, ControlInvocationDrainRequest, ControlRuntimeEffectRequest,
    ControlSurfaceEffectAction, ControlSurfaceEffectRequest,
};
use super::model::{
    corruption_error, ClaimedControlEffect, ControlAppliedEffect, ControlAppliedEffectEvidence,
    ControlEffectClaim, ControlEffectIntent, ControlEffectKind, ControlEffectObservation,
    ControlEffectOutcome, ControlEffectOwner, ControlEffectSubject, ControlSurfaceObservationState,
};
use super::ControlStore;

const MIN_EFFECT_OBSERVATION_BUDGET_MS: u64 = 1_000;

mod contract;

#[cfg(test)]
pub(in crate::control_store) use contract::SystemControlEffectClock;
pub(in crate::control_store) use contract::{
    ControlEffectClock, ControlEffectDispatchRequest, ControlEffectDispatchResult,
    ControlEffectPorts,
};

/// Claims, executes, and observes at most one committed outbox effect.
///
/// The claim transaction is complete before a typed owner port is entered.
/// Provider I/O therefore holds neither a SQLite transaction nor a bounded
/// executor permit. Observation uses a second transaction. If the process
/// exits or persistence fails between those boundaries, explicit
/// reconciliation reuses the same committed idempotency key.
#[derive(Clone)]
pub(in crate::control_store) struct ControlEffectDispatcher {
    store: ControlStore,
    ports: ControlEffectPorts,
    clock: Arc<dyn ControlEffectClock>,
}

impl ControlEffectDispatcher {
    pub(in crate::control_store) fn new(
        store: ControlStore,
        ports: ControlEffectPorts,
        clock: Arc<dyn ControlEffectClock>,
    ) -> Self {
        Self {
            store,
            ports,
            clock,
        }
    }

    pub(in crate::control_store) async fn dispatch_next(
        &self,
        request: ControlEffectDispatchRequest,
    ) -> UseResult<ControlEffectDispatchResult> {
        if request.provider_timeout_ms == 0
            || request
                .provider_timeout_ms
                .checked_add(MIN_EFFECT_OBSERVATION_BUDGET_MS)
                .is_none_or(|bounded| bounded > request.lease_duration_ms)
        {
            return Err(dispatch_error(
                "The provider timeout must leave the minimum observation budget inside the claim lease.",
            ));
        }
        let now_ms = self.clock.now_ms()?;
        let lease_until_ms = now_ms
            .checked_add(request.lease_duration_ms)
            .ok_or_else(|| {
                dispatch_error("The Control effect claim lease timestamp overflowed.")
            })?;
        let claim = ControlEffectClaim {
            operation_id: request.operation_id.clone(),
            worker_id: request.worker_id,
            claim_token: request.claim_token,
            now_ms,
            lease_until_ms,
            explicit_reconciliation: request.explicit_reconciliation,
        };
        let Some(claimed) = self.store.claim_next_effect(claim).await? else {
            return Ok(ControlEffectDispatchResult::Idle);
        };

        let routed = match tokio::time::timeout(
            std::time::Duration::from_millis(request.provider_timeout_ms),
            self.apply_claimed(&request.operation_id, &claimed),
        )
        .await
        {
            Ok(result) => result?,
            Err(_) => RoutedControlEffectOutcome::Unknown(deadline_failure(&claimed)?),
        };
        let observed_at_ms = self.clock.now_ms()?;
        if observed_at_ms < now_ms {
            return Err(dispatch_error(
                "The Control effect observation clock moved backwards after the claim.",
            ));
        }
        let (outcome, application, failure_evidence_digest, error_code) =
            routed.into_observation_parts(&claimed.intent)?;
        let observation = ControlEffectObservation {
            operation_id: request.operation_id,
            idempotency_key: claimed.intent.idempotency_key.clone(),
            claim_token: claimed.claim_token,
            outcome,
            application,
            failure_evidence_digest,
            error_code,
            observed_at_ms,
        };
        let observation_changed = self.store.record_effect_observation(observation).await?;
        Ok(ControlEffectDispatchResult::Observed {
            idempotency_key: claimed.intent.idempotency_key,
            sequence: claimed.intent.sequence,
            attempt: claimed.attempt,
            outcome,
            observation_changed,
        })
    }

    pub(in crate::control_store) async fn apply_claimed(
        &self,
        operation_id: &str,
        claimed: &ClaimedControlEffect,
    ) -> UseResult<RoutedControlEffectOutcome> {
        let identity = request_identity(operation_id, claimed);
        let intent = &claimed.intent;
        match (&intent.owner, &intent.subject, intent.kind) {
            (
                ControlEffectOwner::CapabilityIndex,
                ControlEffectSubject::Installation {
                    expected_capability_generation,
                    capability_generation,
                    descriptor_digest,
                },
                ControlEffectKind::CapabilityCutover,
            ) => {
                let request = ControlCapabilityCutoverRequest {
                    identity,
                    expected_capability_generation: *expected_capability_generation,
                    capability_generation: *capability_generation,
                    descriptor_digest: descriptor_digest.clone(),
                };
                Ok(self
                    .ports
                    .capability_index
                    .cutover(&request)
                    .await
                    .map(
                        |application| ControlAppliedEffectEvidence::CapabilityIndex {
                            capability_generation: request.capability_generation,
                            descriptor_digest: request.descriptor_digest,
                            receipt_digest: application.receipt_digest,
                        },
                    )
                    .into())
            }
            (
                ControlEffectOwner::InvocationLeases,
                ControlEffectSubject::Package {
                    package_id,
                    lifecycle_generation,
                    package_digest,
                    manifest_digest,
                    action,
                },
                ControlEffectKind::CallsDrain,
            ) => {
                let request = ControlInvocationDrainRequest {
                    identity,
                    package_id: package_id.clone(),
                    lifecycle_generation: *lifecycle_generation,
                    package_digest: package_digest.clone(),
                    manifest_digest: manifest_digest.clone(),
                    lifecycle_action: *action,
                };
                Ok(self
                    .ports
                    .invocation_leases
                    .drain(&request)
                    .await
                    .map(
                        |application| ControlAppliedEffectEvidence::InvocationLeases {
                            package_id: request.package_id,
                            lifecycle_generation: request.lifecycle_generation,
                            receipt_digest: application.receipt_digest,
                        },
                    )
                    .into())
            }
            (
                ControlEffectOwner::RuntimeProvider {
                    provider_id,
                    selection_digest,
                },
                ControlEffectSubject::Surface { .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => {
                let surface = surface_request(identity, intent)?;
                let state = observation_state(surface.action);
                let request = ControlRuntimeEffectRequest {
                    surface,
                    provider_id: provider_id.clone(),
                    selection_digest: selection_digest.clone(),
                };
                Ok(self
                    .ports
                    .runtime
                    .apply_surface(&request)
                    .await
                    .map(
                        |application| ControlAppliedEffectEvidence::RuntimeProvider {
                            state,
                            provider_id: request.provider_id,
                            selection_digest: request.selection_digest,
                            receipt_digest: application.receipt_digest,
                            binding: application.binding,
                        },
                    )
                    .into())
            }
            (
                ControlEffectOwner::FlowHost,
                ControlEffectSubject::Surface { .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => {
                let request = surface_request(identity, intent)?;
                let state = observation_state(request.action);
                Ok(self
                    .ports
                    .flow
                    .apply_surface(&request)
                    .await
                    .map(|application| ControlAppliedEffectEvidence::FlowHost {
                        state,
                        receipt_digest: application.receipt_digest,
                        artifact_digest: application.materialization_digest,
                    })
                    .into())
            }
            (
                ControlEffectOwner::KnowledgeHost,
                ControlEffectSubject::Surface { .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => {
                let request = surface_request(identity, intent)?;
                let state = observation_state(request.action);
                Ok(self
                    .ports
                    .knowledge
                    .apply_surface(&request)
                    .await
                    .map(|application| ControlAppliedEffectEvidence::KnowledgeHost {
                        state,
                        receipt_digest: application.receipt_digest,
                        projection_digest: application.materialization_digest,
                    })
                    .into())
            }
            (
                ControlEffectOwner::SkillHost,
                ControlEffectSubject::Surface { .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => {
                let request = surface_request(identity, intent)?;
                let state = observation_state(request.action);
                Ok(self
                    .ports
                    .skill
                    .apply_surface(&request)
                    .await
                    .map(|application| ControlAppliedEffectEvidence::SkillHost {
                        state,
                        receipt_digest: application.receipt_digest,
                        content_digest: application.materialization_digest,
                    })
                    .into())
            }
            (
                ControlEffectOwner::UiHost,
                ControlEffectSubject::Surface { .. },
                ControlEffectKind::SurfacePrepare
                | ControlEffectKind::SurfaceStop
                | ControlEffectKind::SurfaceRemove,
            ) => {
                let request = surface_request(identity, intent)?;
                let state = observation_state(request.action);
                Ok(self
                    .ports
                    .ui
                    .apply_surface(&request)
                    .await
                    .map(|application| ControlAppliedEffectEvidence::UiHost {
                        state,
                        receipt_digest: application.receipt_digest,
                        content_digest: application.materialization_digest,
                    })
                    .into())
            }
            _ => Err(corruption_error(
                "A claimed Control effect does not map to its typed owner port.",
            )),
        }
    }
}

pub(in crate::control_store) enum RoutedControlEffectOutcome {
    Applied(ControlAppliedEffectEvidence),
    Rejected(ControlEffectFailure),
    Unknown(ControlEffectFailure),
}

impl From<ControlEffectPortOutcome<ControlAppliedEffectEvidence>> for RoutedControlEffectOutcome {
    fn from(outcome: ControlEffectPortOutcome<ControlAppliedEffectEvidence>) -> Self {
        match outcome {
            ControlEffectPortOutcome::Applied(application) => Self::Applied(application),
            ControlEffectPortOutcome::Rejected(failure) => Self::Rejected(failure),
            ControlEffectPortOutcome::Unknown(failure) => Self::Unknown(failure),
        }
    }
}

type ObservationParts = (
    ControlEffectOutcome,
    Option<ControlAppliedEffect>,
    Option<String>,
    Option<String>,
);

impl RoutedControlEffectOutcome {
    fn into_observation_parts(self, intent: &ControlEffectIntent) -> UseResult<ObservationParts> {
        match self {
            Self::Applied(evidence) => Ok((
                ControlEffectOutcome::Applied,
                Some(ControlAppliedEffect::new(intent, evidence)?),
                None,
                None,
            )),
            Self::Rejected(failure) => Ok((
                ControlEffectOutcome::Rejected,
                None,
                Some(failure.evidence_digest),
                Some(failure.error_code),
            )),
            Self::Unknown(failure) => Ok((
                ControlEffectOutcome::Unknown,
                None,
                Some(failure.evidence_digest),
                Some(failure.error_code),
            )),
        }
    }
}

fn request_identity(
    operation_id: &str,
    claimed: &ClaimedControlEffect,
) -> ControlEffectRequestIdentity {
    let intent = &claimed.intent;
    ControlEffectRequestIdentity {
        operation_id: operation_id.to_string(),
        installation: intent.installation.clone(),
        plan_digest: intent.plan_digest.clone(),
        operation_action: intent.operation_action,
        installation_generation: intent.installation_generation,
        sequence: intent.sequence,
        idempotency_key: intent.idempotency_key.clone(),
        required: intent.required,
        attempt: claimed.attempt,
        deadline_at_ms: claimed.lease_until_ms,
    }
}

fn surface_request(
    identity: ControlEffectRequestIdentity,
    intent: &ControlEffectIntent,
) -> UseResult<ControlSurfaceEffectRequest> {
    let ControlEffectSubject::Surface {
        package_id,
        lifecycle_generation,
        package_digest,
        manifest_digest,
        action: lifecycle_action,
        surface,
    } = &intent.subject
    else {
        return Err(corruption_error(
            "A surface Control effect has a non-surface subject.",
        ));
    };
    let action = match intent.kind {
        ControlEffectKind::SurfacePrepare => ControlSurfaceEffectAction::Prepare,
        ControlEffectKind::SurfaceStop => ControlSurfaceEffectAction::Stop,
        ControlEffectKind::SurfaceRemove => ControlSurfaceEffectAction::Remove,
        ControlEffectKind::CapabilityCutover | ControlEffectKind::CallsDrain => {
            return Err(corruption_error(
                "A surface Control effect has a non-surface action.",
            ))
        }
    };
    Ok(ControlSurfaceEffectRequest {
        identity,
        package_id: package_id.clone(),
        lifecycle_generation: *lifecycle_generation,
        package_digest: package_digest.clone(),
        manifest_digest: manifest_digest.clone(),
        lifecycle_action: *lifecycle_action,
        surface: surface.clone(),
        action,
    })
}

const fn observation_state(action: ControlSurfaceEffectAction) -> ControlSurfaceObservationState {
    match action {
        ControlSurfaceEffectAction::Prepare => ControlSurfaceObservationState::Prepared,
        ControlSurfaceEffectAction::Stop => ControlSurfaceObservationState::Stopped,
        ControlSurfaceEffectAction::Remove => ControlSurfaceObservationState::Removed,
    }
}

fn dispatch_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.dispatch_invalid", message)
}

fn deadline_failure(claimed: &ClaimedControlEffect) -> UseResult<ControlEffectFailure> {
    const DOMAIN: &[u8] = b"a3s.use.control-effect-provider-timeout.v1\0";
    let mut digest = Sha256::new();
    digest.update(DOMAIN);
    digest.update(claimed.intent.idempotency_key.as_bytes());
    digest.update(claimed.attempt.to_be_bytes());
    ControlEffectFailure::new(
        format!("sha256:{:x}", digest.finalize()),
        "provider.deadline_exceeded",
    )
}
