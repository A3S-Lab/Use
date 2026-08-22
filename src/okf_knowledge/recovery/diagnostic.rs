use std::collections::BTreeSet;

use a3s_use_core::{PlanScope, UseError, UseResult};
use serde::{Deserialize, Serialize};

use super::journal::{
    RestoreOperation, RestoreOperationStatus, MAX_RESTORE_FILE_BYTES,
    MAX_RESTORE_OPERATIONS_PER_SCOPE,
};
use super::{valid_sha256, OkfKnowledgeRecoveryManager};

pub const OKF_KNOWLEDGE_RESTORE_DIAGNOSTIC_SCHEMA: &str =
    "a3s.use.okf-knowledge-restore-diagnostic.v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum OkfKnowledgeRestoreOperationDiagnosticStatus {
    Planned,
    Staged,
    BindingsRestored,
    PriorMoved,
    Published,
    Completed,
}

impl OkfKnowledgeRestoreOperationDiagnosticStatus {
    const fn completed(self) -> bool {
        matches!(self, Self::Completed)
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Staged => "staged",
            Self::BindingsRestored => "bindings-restored",
            Self::PriorMoved => "prior-moved",
            Self::Published => "published",
            Self::Completed => "completed",
        }
    }
}

impl From<RestoreOperationStatus> for OkfKnowledgeRestoreOperationDiagnosticStatus {
    fn from(value: RestoreOperationStatus) -> Self {
        match value {
            RestoreOperationStatus::Planned => Self::Planned,
            RestoreOperationStatus::Staged => Self::Staged,
            RestoreOperationStatus::BindingsRestored => Self::BindingsRestored,
            RestoreOperationStatus::PriorMoved => Self::PriorMoved,
            RestoreOperationStatus::Published => Self::Published,
            RestoreOperationStatus::Completed => Self::Completed,
        }
    }
}

/// Bounded, path-free evidence for one durable database restore operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeRestoreOperationDiagnostic {
    pub scope: PlanScope,
    pub plan_digest: String,
    pub status: OkfKnowledgeRestoreOperationDiagnosticStatus,
    pub backup_database_bytes: u64,
    pub backup_database_sha256: String,
    pub authority_digest: String,
    pub binding_state_digest: String,
    pub registry_generation: u64,
    pub retained_projections: usize,
    pub selected_projections: usize,
    pub missing_bindings: usize,
    pub preserved_prior_files: usize,
    pub started_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

impl OkfKnowledgeRestoreOperationDiagnostic {
    fn from_operation(operation: &RestoreOperation) -> UseResult<Self> {
        operation.validate()?;
        let diagnostic = Self {
            scope: operation.plan.scope.clone(),
            plan_digest: operation.plan_digest.clone(),
            status: operation.status.into(),
            backup_database_bytes: operation.plan.backup.database_bytes,
            backup_database_sha256: operation.plan.backup.database_sha256.clone(),
            authority_digest: operation.plan.authority_digest.clone(),
            binding_state_digest: operation.plan.binding_state_digest.clone(),
            registry_generation: operation.plan.registry_generation,
            retained_projections: operation.plan.retained_projections,
            selected_projections: operation.plan.selected_projections,
            missing_bindings: operation.plan.missing_bindings,
            preserved_prior_files: operation.prior_files.preserved_count(),
            started_at_ms: operation.started_at_ms,
            completed_at_ms: operation.completed_at_ms,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub fn validate(&self) -> UseResult<()> {
        if !valid_machine_id(&self.scope.id)
            || !valid_sha256(&self.plan_digest)
            || self.backup_database_bytes == 0
            || self.backup_database_bytes > MAX_RESTORE_FILE_BYTES
            || !valid_sha256(&self.backup_database_sha256)
            || !valid_sha256(&self.authority_digest)
            || !valid_sha256(&self.binding_state_digest)
            || self.selected_projections > self.retained_projections
            || self.missing_bindings > self.retained_projections
            || self.preserved_prior_files > 3
            || self.started_at_ms == 0
        {
            return Err(diagnostic_error(
                "The Knowledge restore operation diagnostic is invalid or exceeds its bounds.",
            ));
        }
        match (self.status.completed(), self.completed_at_ms) {
            (true, Some(completed_at_ms)) if completed_at_ms >= self.started_at_ms => Ok(()),
            (false, None) => Ok(()),
            _ => Err(diagnostic_error(
                "The Knowledge restore operation status and completion time are inconsistent.",
            )),
        }
    }
}

/// Read-only recovery status for one requested scope plus any global active
/// restore marker. The projection deliberately omits filesystem paths and
/// package-authored content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeRestoreDiagnostic {
    pub schema: String,
    pub scope: PlanScope,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<OkfKnowledgeRestoreOperationDiagnostic>,
    pub retained_operation_directories: usize,
    pub unrecorded_operation_directories: usize,
    pub retention_limit: usize,
    pub retention_remaining: usize,
    pub operations: Vec<OkfKnowledgeRestoreOperationDiagnostic>,
}

impl OkfKnowledgeRestoreDiagnostic {
    pub fn validate(&self) -> UseResult<()> {
        if self.schema != OKF_KNOWLEDGE_RESTORE_DIAGNOSTIC_SCHEMA
            || !valid_machine_id(&self.scope.id)
            || self.retention_limit != MAX_RESTORE_OPERATIONS_PER_SCOPE
            || self.retained_operation_directories > self.retention_limit
            || self.operations.len() > self.retained_operation_directories
            || self.unrecorded_operation_directories
                != self
                    .retained_operation_directories
                    .saturating_sub(self.operations.len())
            || self.retention_remaining
                != self
                    .retention_limit
                    .saturating_sub(self.retained_operation_directories)
        {
            return Err(diagnostic_error(
                "The Knowledge restore diagnostic inventory is inconsistent.",
            ));
        }
        if let Some(active) = &self.active {
            active.validate()?;
        }

        let mut plan_digests = BTreeSet::new();
        for operation in &self.operations {
            operation.validate()?;
            if operation.scope != self.scope || !plan_digests.insert(&operation.plan_digest) {
                return Err(diagnostic_error(
                    "The Knowledge restore diagnostic contains a moved or duplicate operation.",
                ));
            }
        }
        if !self.operations.windows(2).all(|operations| {
            operations[0].started_at_ms > operations[1].started_at_ms
                || operations[0].started_at_ms == operations[1].started_at_ms
                    && operations[0].plan_digest < operations[1].plan_digest
        }) {
            return Err(diagnostic_error(
                "The Knowledge restore diagnostic operations are not in canonical order.",
            ));
        }
        Ok(())
    }
}

impl OkfKnowledgeRecoveryManager {
    /// Inspect bounded restore history without requiring an external backup or
    /// reviewed plan digest. The exclusive maintenance fence makes the
    /// active-marker/journal view coherent. It does not alter restore or
    /// database evidence.
    pub async fn diagnose_restores(
        &self,
        scope: &PlanScope,
    ) -> UseResult<OkfKnowledgeRestoreDiagnostic> {
        if !valid_machine_id(&scope.id) {
            return Err(diagnostic_error(
                "The Knowledge restore diagnostic scope is invalid.",
            ));
        }
        let _maintenance = self.maintenance.acquire_exclusive().await?;
        let marker = self.operations.active().await?;
        let inventory = self.operations.inventory(scope).await?;
        let nonterminal = unique_nonterminal(&inventory.operations)?;

        let active = if let Some(marker) = marker {
            if let Some(scope_operation) = nonterminal {
                if marker.scope != *scope || marker.plan_digest != scope_operation.plan_digest {
                    return Err(diagnostic_error(
                        "The active restore marker conflicts with another nonterminal operation.",
                    ));
                }
            }
            let operation = self
                .operations
                .load(&marker.scope, &marker.plan_digest)
                .await?
                .unwrap_or(marker.operation);
            Some(OkfKnowledgeRestoreOperationDiagnostic::from_operation(
                &operation,
            )?)
        } else {
            nonterminal
                .map(OkfKnowledgeRestoreOperationDiagnostic::from_operation)
                .transpose()?
        };
        let operations = inventory
            .operations
            .iter()
            .map(OkfKnowledgeRestoreOperationDiagnostic::from_operation)
            .collect::<UseResult<Vec<_>>>()?;
        let diagnostic = OkfKnowledgeRestoreDiagnostic {
            schema: OKF_KNOWLEDGE_RESTORE_DIAGNOSTIC_SCHEMA.to_owned(),
            scope: scope.clone(),
            active,
            retained_operation_directories: inventory.directory_count,
            unrecorded_operation_directories: inventory
                .directory_count
                .saturating_sub(operations.len()),
            retention_limit: MAX_RESTORE_OPERATIONS_PER_SCOPE,
            retention_remaining: MAX_RESTORE_OPERATIONS_PER_SCOPE
                .saturating_sub(inventory.directory_count),
            operations,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }
}

fn unique_nonterminal(operations: &[RestoreOperation]) -> UseResult<Option<&RestoreOperation>> {
    let mut nonterminal = None;
    for operation in operations {
        if operation.status != RestoreOperationStatus::Completed
            && nonterminal.replace(operation).is_some()
        {
            return Err(diagnostic_error(
                "More than one nonterminal Knowledge restore exists for one scope.",
            ));
        }
    }
    Ok(nonterminal)
}

fn diagnostic_error(message: impl Into<String>) -> UseError {
    UseError::new("use.okf.knowledge_restore_diagnostic_invalid", message)
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}
