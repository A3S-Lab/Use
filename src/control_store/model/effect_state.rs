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
    pub(in crate::control_store) application: Option<ControlAppliedEffect>,
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
    pub(in crate::control_store) explicit_reconciliation: bool,
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
    pub(in crate::control_store) authority: ControlEffectAuthority,
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
    pub(in crate::control_store) application: Option<ControlAppliedEffect>,
    pub(in crate::control_store) failure_evidence_digest: Option<String>,
    pub(in crate::control_store) error_code: Option<String>,
    pub(in crate::control_store) observed_at_ms: u64,
}

impl ControlEffectObservation {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        let evidence_matches = match self.outcome {
            ControlEffectOutcome::Applied => {
                self.application.as_ref().is_some_and(|application| {
                    application.schema == CONTROL_APPLIED_EFFECT_SCHEMA
                        && application.idempotency_key == self.idempotency_key
                        && application.canonical_bytes().is_ok()
                }) && self.failure_evidence_digest.is_none()
                    && self.error_code.is_none()
            }
            ControlEffectOutcome::Rejected | ControlEffectOutcome::Unknown => {
                self.application.is_none()
                    && self
                        .failure_evidence_digest
                        .as_deref()
                        .is_some_and(valid_sha256)
                    && self.error_code.as_deref().is_some_and(valid_error_code)
            }
        };
        if !valid_machine_id(&self.operation_id)
            || !valid_sha256(&self.idempotency_key)
            || !valid_machine_id(&self.claim_token)
            || self.observed_at_ms == 0
            || !evidence_matches
        {
            return Err(input_error(
                "The Control Store effect observation is invalid.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn evidence_for(
        &self,
        intent: &ControlEffectIntent,
    ) -> UseResult<(Option<Vec<u8>>, String)> {
        self.validate()?;
        match self.outcome {
            ControlEffectOutcome::Applied => {
                let application = self.application.as_ref().ok_or_else(|| {
                    input_error("An applied Control Store effect omitted typed evidence.")
                })?;
                application.validate_for(intent)?;
                Ok((
                    Some(application.canonical_bytes()?),
                    application.descriptor_digest()?,
                ))
            }
            ControlEffectOutcome::Rejected | ControlEffectOutcome::Unknown => Ok((
                None,
                self.failure_evidence_digest.clone().ok_or_else(|| {
                    input_error("A failed Control Store effect omitted diagnostic evidence.")
                })?,
            )),
        }
    }
}
