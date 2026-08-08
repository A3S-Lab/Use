use a3s_use_core::{PlanScope, PluginPackageId, PluginSurfaceRef, UseError, UseResult};
use serde::{Deserialize, Serialize};

use super::{
    PluginLifecycleAction, PluginLifecycleCheckpointOutcome, PluginLifecycleCheckpointReceipt,
    PluginLifecycleFailure, PluginLifecycleOperationRecord, PluginLifecycleOperationStatus,
};

pub const PLUGIN_LIFECYCLE_DIAGNOSTIC_SCHEMA: &str = "a3s.use.plugin-lifecycle-diagnostic.v1";

/// Read-only, secret-free lifecycle evidence for one package and scope.
///
/// The durable journal remains the source of truth. This projection deliberately
/// omits idempotency keys and never includes provider credentials, endpoint
/// tokens, package-authored error text, or secret values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleDiagnostic {
    pub schema: String,
    pub scope: PlanScope,
    pub package_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<PluginLifecycleOperationDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<PluginLifecycleOperationDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleOperationDiagnostic {
    pub operation_id: String,
    pub action: PluginLifecycleAction,
    pub status: PluginLifecycleOperationStatus,
    pub generation: u64,
    pub plan_digest: String,
    pub intent_digest: String,
    pub package_digest: String,
    pub manifest_digest: String,
    pub completed_checkpoints: u32,
    pub total_checkpoints: u32,
    pub checkpoints: Vec<PluginLifecycleCheckpointDiagnostic>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_evidence_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginLifecycleCheckpointDiagnostic {
    pub sequence: u32,
    pub kind: super::PluginLifecycleCheckpointKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub surface: Option<PluginSurfaceRef>,
    pub required: bool,
    pub status: PluginLifecycleCheckpointDiagnosticStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub evidence_digest: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginLifecycleCheckpointDiagnosticStatus {
    Pending,
    Applied,
    OptionalFailed,
    Failed,
}

impl PluginLifecycleDiagnostic {
    pub(super) fn from_records(
        scope: &PlanScope,
        package_id: &str,
        latest: Option<&PluginLifecycleOperationRecord>,
        previous: Option<&PluginLifecycleOperationRecord>,
    ) -> UseResult<Self> {
        super::model::valid_machine_id(&scope.id)
            .then_some(())
            .ok_or_else(|| diagnostic_error("The lifecycle diagnostic scope is invalid."))?;
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The lifecycle diagnostic package identity is invalid.")
        })?;

        for record in latest.into_iter().chain(previous) {
            record.validate()?;
            if record.intent.scope != *scope || record.intent.package_id != package_id {
                return Err(diagnostic_error(
                    "The lifecycle diagnostic record belongs to another scope or package.",
                ));
            }
        }
        if latest
            .zip(previous)
            .is_some_and(|(latest, previous)| latest.intent_digest == previous.intent_digest)
        {
            return Err(diagnostic_error(
                "The latest and previous lifecycle diagnostics cannot identify the same intent.",
            ));
        }

        Ok(Self {
            schema: PLUGIN_LIFECYCLE_DIAGNOSTIC_SCHEMA.to_owned(),
            scope: scope.clone(),
            package_id: package_id.to_owned(),
            latest: latest.map(operation_diagnostic).transpose()?,
            previous: previous.map(operation_diagnostic).transpose()?,
        })
    }
}

fn operation_diagnostic(
    record: &PluginLifecycleOperationRecord,
) -> UseResult<PluginLifecycleOperationDiagnostic> {
    record.validate()?;
    let completed_checkpoints = u32::try_from(record.receipts.len()).map_err(|_| {
        diagnostic_error("The completed lifecycle checkpoint count exceeds its bound.")
    })?;
    let total_checkpoints = u32::try_from(record.intent.checkpoints.len()).map_err(|_| {
        diagnostic_error("The lifecycle checkpoint count exceeds its diagnostic bound.")
    })?;
    let checkpoints = record
        .intent
        .checkpoints
        .iter()
        .enumerate()
        .map(|(index, checkpoint)| {
            checkpoint_diagnostic(
                checkpoint,
                record.receipts.get(index),
                record.last_failure.as_ref(),
            )
        })
        .collect();

    Ok(PluginLifecycleOperationDiagnostic {
        operation_id: record.intent.operation_id.clone(),
        action: record.intent.action,
        status: record.status,
        generation: record.intent.generation,
        plan_digest: record.intent.plan_digest.clone(),
        intent_digest: record.intent_digest.clone(),
        package_digest: record.intent.package_digest.clone(),
        manifest_digest: record.intent.manifest_digest.clone(),
        completed_checkpoints,
        total_checkpoints,
        checkpoints,
        rollback_evidence_digest: record.rollback_evidence_digest.clone(),
        completed_at_ms: record.completed_at_ms,
    })
}

fn checkpoint_diagnostic(
    checkpoint: &super::PluginLifecycleCheckpoint,
    receipt: Option<&PluginLifecycleCheckpointReceipt>,
    failure: Option<&PluginLifecycleFailure>,
) -> PluginLifecycleCheckpointDiagnostic {
    let (status, evidence_digest, error_code, observed_at_ms) = if let Some(receipt) = receipt {
        let status = match receipt.outcome {
            PluginLifecycleCheckpointOutcome::Applied => {
                PluginLifecycleCheckpointDiagnosticStatus::Applied
            }
            PluginLifecycleCheckpointOutcome::OptionalFailed => {
                PluginLifecycleCheckpointDiagnosticStatus::OptionalFailed
            }
        };
        (
            status,
            Some(receipt.evidence_digest.clone()),
            receipt.error_code.clone(),
            Some(receipt.completed_at_ms),
        )
    } else if let Some(failure) = failure.filter(|failure| failure.sequence == checkpoint.sequence)
    {
        (
            PluginLifecycleCheckpointDiagnosticStatus::Failed,
            Some(failure.evidence_digest.clone()),
            Some(failure.error_code.clone()),
            Some(failure.failed_at_ms),
        )
    } else {
        (
            PluginLifecycleCheckpointDiagnosticStatus::Pending,
            None,
            None,
            None,
        )
    };

    PluginLifecycleCheckpointDiagnostic {
        sequence: checkpoint.sequence,
        kind: checkpoint.kind,
        surface: checkpoint.surface.clone(),
        required: checkpoint.required,
        status,
        evidence_digest,
        error_code,
        observed_at_ms,
    }
}

fn diagnostic_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.lifecycle_diagnostic_invalid", message)
}
