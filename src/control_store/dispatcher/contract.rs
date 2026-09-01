use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{UseError, UseResult};

use super::super::effect_port::{
    ControlCapabilityIndexEffectPort, ControlFlowEffectPort, ControlInvocationLeaseEffectPort,
    ControlKnowledgeEffectPort, ControlRuntimeEffectPort, ControlSkillEffectPort,
    ControlUiEffectPort,
};
use super::super::model::ControlEffectOutcome;

pub(in crate::control_store) trait ControlEffectClock:
    Send + Sync
{
    fn now_ms(&self) -> UseResult<u64>;
}

#[derive(Debug, Default)]
pub(in crate::control_store) struct SystemControlEffectClock;

impl ControlEffectClock for SystemControlEffectClock {
    fn now_ms(&self) -> UseResult<u64> {
        let elapsed = SystemTime::now().duration_since(UNIX_EPOCH).map_err(|_| {
            UseError::new(
                "use.control_store.clock_invalid",
                "The system clock predates the Unix epoch.",
            )
        })?;
        u64::try_from(elapsed.as_millis()).map_err(|_| {
            UseError::new(
                "use.control_store.clock_invalid",
                "The system clock exceeds the Control Store timestamp range.",
            )
        })
    }
}

#[derive(Clone)]
pub(in crate::control_store) struct ControlEffectPorts {
    pub(super) capability_index: Arc<dyn ControlCapabilityIndexEffectPort>,
    pub(super) invocation_leases: Arc<dyn ControlInvocationLeaseEffectPort>,
    pub(super) runtime: Arc<dyn ControlRuntimeEffectPort>,
    pub(super) flow: Arc<dyn ControlFlowEffectPort>,
    pub(super) knowledge: Arc<dyn ControlKnowledgeEffectPort>,
    pub(super) skill: Arc<dyn ControlSkillEffectPort>,
    pub(super) ui: Arc<dyn ControlUiEffectPort>,
}

impl ControlEffectPorts {
    #[allow(clippy::too_many_arguments)]
    pub(in crate::control_store) fn new(
        capability_index: Arc<dyn ControlCapabilityIndexEffectPort>,
        invocation_leases: Arc<dyn ControlInvocationLeaseEffectPort>,
        runtime: Arc<dyn ControlRuntimeEffectPort>,
        flow: Arc<dyn ControlFlowEffectPort>,
        knowledge: Arc<dyn ControlKnowledgeEffectPort>,
        skill: Arc<dyn ControlSkillEffectPort>,
        ui: Arc<dyn ControlUiEffectPort>,
    ) -> Self {
        Self {
            capability_index,
            invocation_leases,
            runtime,
            flow,
            knowledge,
            skill,
            ui,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectDispatchRequest {
    pub(in crate::control_store) operation_id: String,
    pub(in crate::control_store) worker_id: String,
    pub(in crate::control_store) claim_token: String,
    pub(in crate::control_store) lease_duration_ms: u64,
    pub(in crate::control_store) provider_timeout_ms: u64,
    pub(in crate::control_store) deferred_retry_delay_ms: u64,
    pub(in crate::control_store) explicit_reconciliation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) enum ControlEffectDispatchResult {
    Idle,
    Observed {
        idempotency_key: String,
        sequence: u32,
        attempt: u32,
        outcome: ControlEffectOutcome,
        retry_not_before_ms: Option<u64>,
        observation_changed: bool,
    },
}
