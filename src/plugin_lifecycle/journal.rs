use serde::{Deserialize, Serialize};

use a3s_use_core::{UseError, UseResult};

use super::model::{lifecycle_error, valid_sha256, PluginLifecycleIntent};

pub const PLUGIN_LIFECYCLE_OPERATION_SCHEMA: &str = "a3s.use.plugin-lifecycle-operation.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycleOperationStatus {
    Applying,
    RollingBack,
    Completed,
    RolledBack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycleCheckpointOutcome {
    Applied,
    OptionalFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleCheckpointReceipt {
    pub sequence: u32,
    pub idempotency_key: String,
    pub outcome: PluginLifecycleCheckpointOutcome,
    pub evidence_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub completed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleFailure {
    pub sequence: u32,
    pub idempotency_key: String,
    pub error_code: String,
    pub evidence_digest: String,
    pub failed_at_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleOperationRecord {
    pub schema: String,
    pub intent: PluginLifecycleIntent,
    pub intent_digest: String,
    pub status: PluginLifecycleOperationStatus,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub receipts: Vec<PluginLifecycleCheckpointReceipt>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_failure: Option<PluginLifecycleFailure>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_evidence_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

impl PluginLifecycleOperationRecord {
    pub fn new(intent: PluginLifecycleIntent) -> UseResult<Self> {
        intent.validate()?;
        let record = Self {
            schema: PLUGIN_LIFECYCLE_OPERATION_SCHEMA.to_string(),
            intent_digest: intent.descriptor_digest()?,
            intent,
            status: PluginLifecycleOperationStatus::Applying,
            receipts: Vec::new(),
            last_failure: None,
            rollback_evidence_digest: None,
            completed_at_ms: None,
        };
        record.validate()?;
        Ok(record)
    }

    pub fn validate(&self) -> UseResult<()> {
        self.intent.validate()?;
        if self.schema != PLUGIN_LIFECYCLE_OPERATION_SCHEMA
            || self.intent_digest != self.intent.descriptor_digest()?
            || self.receipts.len() > self.intent.checkpoints.len()
        {
            return Err(operation_error(
                "The cognitive-package lifecycle operation identity is invalid.",
            ));
        }

        let mut previous_time = 0;
        for (index, receipt) in self.receipts.iter().enumerate() {
            let checkpoint = &self.intent.checkpoints[index];
            if receipt.sequence != checkpoint.sequence
                || receipt.idempotency_key != checkpoint.idempotency_key
                || !valid_sha256(&receipt.evidence_digest)
                || receipt.completed_at_ms == 0
                || receipt.completed_at_ms < previous_time
            {
                return Err(operation_error(
                    "A lifecycle checkpoint receipt does not match its canonical sequence.",
                ));
            }
            match (receipt.outcome, checkpoint.required, &receipt.error_code) {
                (PluginLifecycleCheckpointOutcome::Applied, _, None) => {}
                (PluginLifecycleCheckpointOutcome::OptionalFailed, false, Some(code))
                    if valid_error_code(code) => {}
                _ => return Err(operation_error(
                    "A lifecycle checkpoint outcome is inconsistent with required-surface policy.",
                )),
            }
            previous_time = receipt.completed_at_ms;
        }

        if let Some(failure) = &self.last_failure {
            let checkpoint = self.next_checkpoint().ok_or_else(|| {
                operation_error("A completed checkpoint sequence cannot retain a failure.")
            })?;
            if !matches!(
                self.status,
                PluginLifecycleOperationStatus::Applying
                    | PluginLifecycleOperationStatus::RollingBack
                    | PluginLifecycleOperationStatus::RolledBack
            ) || failure.sequence != checkpoint.sequence
                || failure.idempotency_key != checkpoint.idempotency_key
                || !valid_error_code(&failure.error_code)
                || !valid_sha256(&failure.evidence_digest)
                || failure.failed_at_ms == 0
                || failure.failed_at_ms < previous_time
            {
                return Err(operation_error(
                    "The lifecycle failure does not bind the exact next checkpoint.",
                ));
            }
        }

        match self.status {
            PluginLifecycleOperationStatus::Applying
                if self.completed_at_ms.is_none() && self.rollback_evidence_digest.is_none() => {}
            PluginLifecycleOperationStatus::RollingBack
                if self.completed_at_ms.is_none() && self.rollback_evidence_digest.is_none() => {}
            PluginLifecycleOperationStatus::Completed
                if self.receipts.len() == self.intent.checkpoints.len()
                    && self.last_failure.is_none()
                    && self.rollback_evidence_digest.is_none()
                    && self
                        .completed_at_ms
                        .is_some_and(|time| time >= previous_time && time > 0) => {}
            PluginLifecycleOperationStatus::RolledBack
                if self.receipts.len() < self.intent.checkpoints.len()
                    && self
                        .rollback_evidence_digest
                        .as_deref()
                        .is_some_and(valid_sha256)
                    && self.completed_at_ms.is_some_and(|time| {
                        time >= previous_time
                            && time
                                >= self
                                    .last_failure
                                    .as_ref()
                                    .map_or(0, |failure| failure.failed_at_ms)
                            && time > 0
                    }) => {}
            _ => {
                return Err(operation_error(
                    "The lifecycle operation status does not match its durable checkpoints.",
                ))
            }
        }
        Ok(())
    }

    pub fn next_checkpoint(&self) -> Option<&super::PluginLifecycleCheckpoint> {
        self.intent.checkpoints.get(self.receipts.len())
    }

    pub fn record_checkpoint(
        &mut self,
        idempotency_key: &str,
        outcome: PluginLifecycleCheckpointOutcome,
        evidence_digest: impl Into<String>,
        error_code: Option<String>,
        completed_at_ms: u64,
    ) -> UseResult<bool> {
        self.validate()?;
        if self.status != PluginLifecycleOperationStatus::Applying {
            return Err(operation_conflict(
                "A non-applying lifecycle operation cannot record forward progress.",
            ));
        }
        let evidence_digest = evidence_digest.into();
        if let Some(receipt) = self
            .receipts
            .iter()
            .find(|receipt| receipt.idempotency_key == idempotency_key)
        {
            if receipt.outcome == outcome
                && receipt.evidence_digest == evidence_digest
                && receipt.error_code == error_code
            {
                return Ok(false);
            }
            return Err(operation_conflict(
                "A completed lifecycle checkpoint was replayed with different evidence.",
            ));
        }
        let checkpoint = self.next_checkpoint().ok_or_else(|| {
            operation_conflict("The lifecycle operation has no pending checkpoint.")
        })?;
        if checkpoint.idempotency_key != idempotency_key {
            return Err(operation_conflict(
                "Lifecycle checkpoints must complete in canonical dependency order.",
            ));
        }
        let receipt = PluginLifecycleCheckpointReceipt {
            sequence: checkpoint.sequence,
            idempotency_key: checkpoint.idempotency_key.clone(),
            outcome,
            evidence_digest,
            error_code,
            completed_at_ms,
        };
        let mut candidate = self.clone();
        candidate.receipts.push(receipt);
        candidate.last_failure = None;
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    pub fn record_failure(
        &mut self,
        idempotency_key: &str,
        error_code: impl Into<String>,
        evidence_digest: impl Into<String>,
        failed_at_ms: u64,
    ) -> UseResult<()> {
        self.validate()?;
        if self.status != PluginLifecycleOperationStatus::Applying {
            return Err(operation_conflict(
                "A non-applying lifecycle operation cannot record a forward failure.",
            ));
        }
        let checkpoint = self.next_checkpoint().ok_or_else(|| {
            operation_conflict("The lifecycle operation has no pending checkpoint.")
        })?;
        if checkpoint.idempotency_key != idempotency_key {
            return Err(operation_conflict(
                "A lifecycle failure must bind the exact next checkpoint.",
            ));
        }
        let mut candidate = self.clone();
        candidate.last_failure = Some(PluginLifecycleFailure {
            sequence: checkpoint.sequence,
            idempotency_key: checkpoint.idempotency_key.clone(),
            error_code: error_code.into(),
            evidence_digest: evidence_digest.into(),
            failed_at_ms,
        });
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    pub fn complete(&mut self, completed_at_ms: u64) -> UseResult<bool> {
        self.validate()?;
        if self.status == PluginLifecycleOperationStatus::Completed {
            return Ok(false);
        }
        if self.status != PluginLifecycleOperationStatus::Applying
            || self.next_checkpoint().is_some()
            || self.last_failure.is_some()
        {
            return Err(operation_conflict(
                "A lifecycle operation cannot complete before every checkpoint succeeds or records an optional failure.",
            ));
        }
        let mut candidate = self.clone();
        candidate.status = PluginLifecycleOperationStatus::Completed;
        candidate.completed_at_ms = Some(completed_at_ms);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    pub fn start_rollback(&mut self) -> UseResult<bool> {
        self.validate()?;
        if self.status == PluginLifecycleOperationStatus::RollingBack {
            return Ok(false);
        }
        if self.status != PluginLifecycleOperationStatus::Applying
            || self.receipts.len() == self.intent.checkpoints.len()
        {
            return Err(operation_conflict(
                "Only an unpublished applying lifecycle operation can start rollback.",
            ));
        }
        let mut candidate = self.clone();
        candidate.status = PluginLifecycleOperationStatus::RollingBack;
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    pub fn roll_back(
        &mut self,
        evidence_digest: impl Into<String>,
        completed_at_ms: u64,
    ) -> UseResult<bool> {
        self.validate()?;
        let evidence_digest = evidence_digest.into();
        if self.status == PluginLifecycleOperationStatus::RolledBack {
            if self.rollback_evidence_digest.as_deref() == Some(&evidence_digest) {
                return Ok(false);
            }
            return Err(operation_conflict(
                "A rolled-back lifecycle operation was replayed with different evidence.",
            ));
        }
        if self.status != PluginLifecycleOperationStatus::RollingBack
            || self.receipts.len() == self.intent.checkpoints.len()
            || !valid_sha256(&evidence_digest)
        {
            return Err(operation_conflict(
                "Only an unpublished rolling-back lifecycle operation can complete rollback.",
            ));
        }
        let mut candidate = self.clone();
        candidate.status = PluginLifecycleOperationStatus::RolledBack;
        candidate.rollback_evidence_digest = Some(evidence_digest);
        candidate.completed_at_ms = Some(completed_at_ms);
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'.' | b'_' | b'-')
        })
}

fn operation_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_operation_invalid", message)
}

fn operation_conflict(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_operation_conflict", message)
}

pub(super) fn record_error(message: impl Into<String>) -> UseError {
    let error = lifecycle_error(message);
    UseError::new("use.plugin.lifecycle_record_invalid", error.message)
}
