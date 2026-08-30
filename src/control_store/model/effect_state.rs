use serde::{Deserialize, Serialize};

use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlEffectStatus {
    Pending,
    Claimed,
    Applied,
    Rejected,
    Unknown,
}

impl ControlEffectStatus {
    pub(in crate::control_store) const fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Claimed => "claimed",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Unknown => "unknown",
        }
    }

    pub(in crate::control_store) fn parse(value: &str) -> UseResult<Self> {
        match value {
            "pending" => Ok(Self::Pending),
            "claimed" => Ok(Self::Claimed),
            "applied" => Ok(Self::Applied),
            "rejected" => Ok(Self::Rejected),
            "unknown" => Ok(Self::Unknown),
            _ => Err(corruption_error(
                "A Control Store effect status is invalid.",
            )),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlEffectRecord {
    pub(in crate::control_store) operation_id: String,
    pub(in crate::control_store) intent: ControlEffectIntent,
    pub(in crate::control_store) payload_digest: String,
    pub(in crate::control_store) status: ControlEffectStatus,
    pub(in crate::control_store) attempt: u32,
    pub(in crate::control_store) claim_owner: Option<String>,
    pub(in crate::control_store) claim_token: Option<String>,
    pub(in crate::control_store) lease_until_ms: Option<u64>,
    pub(in crate::control_store) evidence_digest: Option<String>,
    pub(in crate::control_store) error_code: Option<String>,
    pub(in crate::control_store) observed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectClaim {
    pub(in crate::control_store) operation_id: String,
    pub(in crate::control_store) worker_id: String,
    pub(in crate::control_store) claim_token: String,
    pub(in crate::control_store) now_ms: u64,
    pub(in crate::control_store) lease_until_ms: u64,
    pub(in crate::control_store) reconcile_unknown: bool,
}

impl ControlEffectClaim {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.operation_id)
            || !valid_machine_id(&self.worker_id)
            || !valid_machine_id(&self.claim_token)
            || self.now_ms == 0
            || self.lease_until_ms <= self.now_ms
            || self.lease_until_ms - self.now_ms > MAX_EFFECT_LEASE_MS
        {
            return Err(input_error("The Control Store effect claim is invalid."));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ClaimedControlEffect {
    pub(in crate::control_store) intent: ControlEffectIntent,
    pub(in crate::control_store) attempt: u32,
    pub(in crate::control_store) claim_token: String,
    pub(in crate::control_store) lease_until_ms: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::control_store) enum ControlEffectOutcome {
    Applied,
    Rejected,
    Unknown,
}

impl ControlEffectOutcome {
    pub(in crate::control_store) const fn status(self) -> ControlEffectStatus {
        match self {
            Self::Applied => ControlEffectStatus::Applied,
            Self::Rejected => ControlEffectStatus::Rejected,
            Self::Unknown => ControlEffectStatus::Unknown,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlEffectObservation {
    pub(in crate::control_store) operation_id: String,
    pub(in crate::control_store) idempotency_key: String,
    pub(in crate::control_store) claim_token: String,
    pub(in crate::control_store) outcome: ControlEffectOutcome,
    pub(in crate::control_store) evidence_digest: String,
    pub(in crate::control_store) error_code: Option<String>,
    pub(in crate::control_store) observed_at_ms: u64,
}

impl ControlEffectObservation {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        let error_matches = match self.outcome {
            ControlEffectOutcome::Applied => self.error_code.is_none(),
            ControlEffectOutcome::Rejected | ControlEffectOutcome::Unknown => {
                self.error_code.as_deref().is_some_and(valid_error_code)
            }
        };
        if !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.idempotency_key)
            || !valid_machine_id(&self.claim_token)
            || !valid_sha256(&self.evidence_digest)
            || self.observed_at_ms == 0
            || !error_matches
        {
            return Err(input_error(
                "The Control Store effect observation is invalid.",
            ));
        }
        Ok(())
    }
}
