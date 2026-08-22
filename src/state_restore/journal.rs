use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, ACTIVE_STATE_RESTORE_MARKER};
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::{canonical_json, sha256, valid_sha256, StateRestoreActionSummary, StateRestorePlan};

mod history;
mod storage;

use storage::{
    discard_unpublished_temporary_json, ensure_owned_directory, read_optional_json,
    recover_temporary_json, sync_directory, validate_directory_chain, write_json,
};

pub const A3S_USE_STATE_RESTORE_OPERATION_SCHEMA: &str = "a3s.use.state-restore-operation.v1";
pub const A3S_USE_STATE_RESTORE_RESULT_SCHEMA: &str = "a3s.use.state-restore-result.v1";
const ACTIVE_STATE_RESTORE_SCHEMA: &str = "a3s.use.active-state-restore.v3";
const MAX_OPERATION_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MARKER_BYTES: u64 = 4 * 1024;
pub(super) const MAX_OPERATION_COUNT: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum StateRestoreOperationStatus {
    Planned,
    Staged,
    Publishing,
    Published,
    CandidatesRemoved,
    Verified,
    Completed,
}

impl StateRestoreOperationStatus {
    fn sequence(self) -> u8 {
        match self {
            Self::Planned => 0,
            Self::Staged => 1,
            Self::Publishing => 2,
            Self::Published => 3,
            Self::CandidatesRemoved => 4,
            Self::Verified => 5,
            Self::Completed => 6,
        }
    }

    pub(super) const fn checkpoint(self) -> &'static str {
        match self {
            Self::Planned => "status-planned",
            Self::Staged => "status-staged",
            Self::Publishing => "status-publishing",
            Self::Published => "status-published",
            Self::CandidatesRemoved => "status-candidates-removed",
            Self::Verified => "status-verified",
            Self::Completed => "status-completed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct StateRestoreOperation {
    pub(super) schema: String,
    pub(super) plan: StateRestorePlan,
    pub(super) plan_digest: String,
    pub(super) rollback_backup_manifest_digest: String,
    pub(super) status: StateRestoreOperationStatus,
    pub(super) started_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) completed_at_ms: Option<u64>,
}

impl StateRestoreOperation {
    pub(super) fn new(
        plan: StateRestorePlan,
        plan_digest: String,
        rollback_backup_manifest_digest: String,
        started_at_ms: u64,
    ) -> UseResult<Self> {
        let operation = Self {
            schema: A3S_USE_STATE_RESTORE_OPERATION_SCHEMA.to_owned(),
            plan,
            plan_digest,
            rollback_backup_manifest_digest,
            status: StateRestoreOperationStatus::Planned,
            started_at_ms,
            completed_at_ms: None,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        self.plan.validate()?;
        if self.schema != A3S_USE_STATE_RESTORE_OPERATION_SCHEMA
            || self.plan.status != super::StateRestorePlanStatus::Required
            || !valid_sha256(&self.plan_digest)
            || self.plan.descriptor_digest()? != self.plan_digest
            || !valid_sha256(&self.rollback_backup_manifest_digest)
            || self.started_at_ms == 0
        {
            return Err(operation_invalid(
                "The whole-installation restore operation identity is invalid.",
            ));
        }
        match (self.status, self.completed_at_ms) {
            (StateRestoreOperationStatus::Completed, Some(completed))
                if completed >= self.started_at_ms =>
            {
                Ok(())
            }
            (StateRestoreOperationStatus::Completed, _) => Err(operation_invalid(
                "A completed whole-installation restore has no valid completion time.",
            )),
            (_, None) => Ok(()),
            (_, Some(_)) => Err(operation_invalid(
                "A nonterminal whole-installation restore carries a completion time.",
            )),
        }
    }

    pub(super) fn advance(
        &mut self,
        next: StateRestoreOperationStatus,
        completed_at_ms: Option<u64>,
    ) -> UseResult<()> {
        self.validate()?;
        if next.sequence() != self.status.sequence().saturating_add(1) {
            return Err(operation_conflict(
                "Whole-installation restore checkpoints must advance in canonical order.",
            ));
        }
        let mut candidate = self.clone();
        candidate.status = next;
        candidate.completed_at_ms = completed_at_ms;
        candidate.validate()?;
        *self = candidate;
        Ok(())
    }

    fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(sha256(&canonical_json(self, "state restore operation")?))
    }

    pub(super) fn result(&self) -> UseResult<StateRestoreResult> {
        self.validate()?;
        if self.status != StateRestoreOperationStatus::Completed {
            return Err(operation_conflict(
                "A nonterminal whole-installation restore has no final result.",
            ));
        }
        let result = StateRestoreResult {
            schema: A3S_USE_STATE_RESTORE_RESULT_SCHEMA.to_owned(),
            changed: true,
            plan_digest: self.plan_digest.clone(),
            backup_manifest_digest: self.plan.backup_manifest_digest.clone(),
            before_inventory_digest: self.plan.before_inventory_digest.clone(),
            after_inventory_digest: self.plan.backup.inventory_digest.clone(),
            rollback_backup_manifest_digest: Some(self.rollback_backup_manifest_digest.clone()),
            summary: self.plan.summary.clone(),
            completed_at_ms: self.completed_at_ms,
        };
        result.validate()?;
        Ok(result)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestoreResult {
    pub schema: String,
    pub changed: bool,
    pub plan_digest: String,
    pub backup_manifest_digest: String,
    pub before_inventory_digest: String,
    pub after_inventory_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rollback_backup_manifest_digest: Option<String>,
    pub summary: StateRestoreActionSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

impl StateRestoreResult {
    pub(super) fn no_change(plan: &StateRestorePlan, plan_digest: String) -> UseResult<Self> {
        plan.validate()?;
        let result = Self {
            schema: A3S_USE_STATE_RESTORE_RESULT_SCHEMA.to_owned(),
            changed: false,
            plan_digest,
            backup_manifest_digest: plan.backup_manifest_digest.clone(),
            before_inventory_digest: plan.before_inventory_digest.clone(),
            after_inventory_digest: plan.backup.inventory_digest.clone(),
            rollback_backup_manifest_digest: None,
            summary: plan.summary.clone(),
            completed_at_ms: None,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> UseResult<()> {
        let digests_valid = [
            self.plan_digest.as_str(),
            self.backup_manifest_digest.as_str(),
            self.before_inventory_digest.as_str(),
            self.after_inventory_digest.as_str(),
        ]
        .into_iter()
        .all(valid_sha256)
            && self
                .rollback_backup_manifest_digest
                .as_deref()
                .is_none_or(valid_sha256);
        if self.schema != A3S_USE_STATE_RESTORE_RESULT_SCHEMA || !digests_valid {
            return Err(operation_invalid(
                "The whole-installation restore result identity is invalid.",
            ));
        }
        match (
            self.changed,
            &self.rollback_backup_manifest_digest,
            self.completed_at_ms,
        ) {
            (true, Some(_), Some(completed)) if completed > 0 => Ok(()),
            (false, None, None)
                if self.before_inventory_digest == self.after_inventory_digest
                    && self.summary.add_files == 0
                    && self.summary.replace_files == 0
                    && self.summary.remove_files == 0 =>
            {
                Ok(())
            }
            _ => Err(operation_invalid(
                "The whole-installation restore result does not match its terminal outcome.",
            )),
        }
    }
}

pub const A3S_USE_STATE_RESTORE_DIAGNOSTIC_SCHEMA: &str = "a3s.use.state-restore-diagnostic.v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum StateRestoreDiagnosticStatus {
    MarkerOnly,
    Planned,
    Staged,
    Publishing,
    Published,
    CandidatesRemoved,
    Verified,
    Completed,
}

impl From<StateRestoreOperationStatus> for StateRestoreDiagnosticStatus {
    fn from(status: StateRestoreOperationStatus) -> Self {
        match status {
            StateRestoreOperationStatus::Planned => Self::Planned,
            StateRestoreOperationStatus::Staged => Self::Staged,
            StateRestoreOperationStatus::Publishing => Self::Publishing,
            StateRestoreOperationStatus::Published => Self::Published,
            StateRestoreOperationStatus::CandidatesRemoved => Self::CandidatesRemoved,
            StateRestoreOperationStatus::Verified => Self::Verified,
            StateRestoreOperationStatus::Completed => Self::Completed,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ActiveStateRestoreDiagnostic {
    pub plan_digest: String,
    pub rollback_backup_manifest_digest: String,
    pub status: StateRestoreDiagnosticStatus,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestoreOperationDiagnostic {
    pub plan_digest: String,
    pub status: StateRestoreDiagnosticStatus,
    pub backup_manifest_digest: String,
    pub before_inventory_digest: String,
    pub after_inventory_digest: String,
    pub rollback_backup_manifest_digest: String,
    pub summary: StateRestoreActionSummary,
    pub started_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

impl From<&StateRestoreOperation> for StateRestoreOperationDiagnostic {
    fn from(operation: &StateRestoreOperation) -> Self {
        Self {
            plan_digest: operation.plan_digest.clone(),
            status: operation.status.into(),
            backup_manifest_digest: operation.plan.backup_manifest_digest.clone(),
            before_inventory_digest: operation.plan.before_inventory_digest.clone(),
            after_inventory_digest: operation.plan.backup.inventory_digest.clone(),
            rollback_backup_manifest_digest: operation.rollback_backup_manifest_digest.clone(),
            summary: operation.plan.summary.clone(),
            started_at_ms: operation.started_at_ms,
            completed_at_ms: operation.completed_at_ms,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StateRestoreDiagnostic {
    pub schema: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<ActiveStateRestoreDiagnostic>,
    pub operations: Vec<StateRestoreOperationDiagnostic>,
    pub retained_operation_directories: usize,
    pub unrecorded_operation_directories: usize,
    pub retention_limit: usize,
    pub retention_remaining: usize,
}

impl StateRestoreDiagnostic {
    pub fn validate(&self) -> UseResult<()> {
        let counts_valid = self.retention_limit == MAX_OPERATION_COUNT
            && self.retained_operation_directories <= self.retention_limit
            && self.unrecorded_operation_directories <= self.retained_operation_directories
            && self.operations.len() + self.unrecorded_operation_directories
                == self.retained_operation_directories
            && self.retention_remaining + self.retained_operation_directories
                == self.retention_limit;
        let operations_valid = self.operations.iter().all(|operation| {
            valid_sha256(&operation.plan_digest)
                && valid_sha256(&operation.backup_manifest_digest)
                && valid_sha256(&operation.before_inventory_digest)
                && valid_sha256(&operation.after_inventory_digest)
                && valid_sha256(&operation.rollback_backup_manifest_digest)
                && operation.started_at_ms > 0
        });
        let active_valid = self.active.as_ref().is_none_or(|active| {
            valid_sha256(&active.plan_digest)
                && valid_sha256(&active.rollback_backup_manifest_digest)
        });
        if self.schema != A3S_USE_STATE_RESTORE_DIAGNOSTIC_SCHEMA
            || !counts_valid
            || !operations_valid
            || !active_valid
        {
            return Err(operation_invalid(
                "The whole-installation restore diagnostic is internally inconsistent.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ActiveStateRestoreMarker {
    schema: String,
    pub(super) plan_digest: String,
    operation_digest: String,
    pub(super) rollback_backup_manifest_digest: String,
    started_at_ms: u64,
}

impl ActiveStateRestoreMarker {
    fn new(operation: &StateRestoreOperation) -> UseResult<Self> {
        let marker = Self {
            schema: ACTIVE_STATE_RESTORE_SCHEMA.to_owned(),
            plan_digest: operation.plan_digest.clone(),
            operation_digest: initial_operation_digest(operation)?,
            rollback_backup_manifest_digest: operation.rollback_backup_manifest_digest.clone(),
            started_at_ms: operation.started_at_ms,
        };
        marker.validate()?;
        Ok(marker)
    }

    fn validate(&self) -> UseResult<()> {
        if self.schema != ACTIVE_STATE_RESTORE_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || !valid_sha256(&self.operation_digest)
            || !valid_sha256(&self.rollback_backup_manifest_digest)
            || self.started_at_ms == 0
        {
            return Err(operation_invalid(
                "The active whole-installation restore marker is invalid.",
            ));
        }
        Ok(())
    }

    fn binds_operation(&self, operation: &StateRestoreOperation) -> UseResult<bool> {
        self.validate()?;
        operation.validate()?;
        Ok(self.plan_digest == operation.plan_digest
            && self.rollback_backup_manifest_digest == operation.rollback_backup_manifest_digest
            && self.started_at_ms == operation.started_at_ms
            && self.operation_digest == initial_operation_digest(operation)?)
    }

    pub(super) fn recover_operation(
        &self,
        plan: StateRestorePlan,
    ) -> UseResult<StateRestoreOperation> {
        self.validate()?;
        let operation = StateRestoreOperation::new(
            plan,
            self.plan_digest.clone(),
            self.rollback_backup_manifest_digest.clone(),
            self.started_at_ms,
        )?;
        if operation.descriptor_digest()? != self.operation_digest {
            return Err(operation_invalid(
                "The active restore marker does not match the reconstructed operation.",
            ));
        }
        Ok(operation)
    }
}

fn initial_operation_digest(operation: &StateRestoreOperation) -> UseResult<String> {
    operation.validate()?;
    let mut initial = operation.clone();
    initial.status = StateRestoreOperationStatus::Planned;
    initial.completed_at_ms = None;
    initial.descriptor_digest()
}

#[derive(Debug, Clone)]
pub(super) struct StateRestoreOperationStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl StateRestoreOperationStore {
    pub(super) fn new(paths: ExtensionPaths) -> Self {
        let state_root = paths.state_root().to_path_buf();
        Self {
            root: state_root.join("operations").join("state-restores"),
            state_root,
        }
    }

    pub(super) async fn active(&self) -> UseResult<Option<ActiveStateRestoreMarker>> {
        let path = self.state_root.join(ACTIVE_STATE_RESTORE_MARKER);
        discard_unpublished_temporary_json(&path, MAX_MARKER_BYTES).await?;
        let marker: Option<ActiveStateRestoreMarker> =
            read_optional_json(&path, MAX_MARKER_BYTES, "active state restore marker").await?;
        marker
            .map(|marker| {
                marker.validate()?;
                Ok(marker)
            })
            .transpose()
    }

    pub(super) async fn diagnose(&self) -> UseResult<StateRestoreDiagnostic> {
        let marker: Option<ActiveStateRestoreMarker> = read_optional_json(
            &self.state_root.join(ACTIVE_STATE_RESTORE_MARKER),
            MAX_MARKER_BYTES,
            "active state restore marker",
        )
        .await?;
        if let Some(marker) = &marker {
            marker.validate()?;
        }

        let inventory = history::inspect(self).await?;
        let retained_operation_directories = inventory.retained_directories;
        let unrecorded_operation_directories = inventory.unrecorded_directories;
        let mut operations = inventory
            .operations
            .iter()
            .map(StateRestoreOperationDiagnostic::from)
            .collect::<Vec<_>>();
        operations.sort_by(|left, right| {
            right
                .started_at_ms
                .cmp(&left.started_at_ms)
                .then_with(|| left.plan_digest.cmp(&right.plan_digest))
        });
        let active = marker.map(|marker| {
            let status = operations
                .iter()
                .find(|operation| operation.plan_digest == marker.plan_digest)
                .map_or(StateRestoreDiagnosticStatus::MarkerOnly, |operation| {
                    operation.status
                });
            ActiveStateRestoreDiagnostic {
                plan_digest: marker.plan_digest,
                rollback_backup_manifest_digest: marker.rollback_backup_manifest_digest,
                status,
            }
        });
        let diagnostic = StateRestoreDiagnostic {
            schema: A3S_USE_STATE_RESTORE_DIAGNOSTIC_SCHEMA.to_owned(),
            active,
            operations,
            retained_operation_directories,
            unrecorded_operation_directories,
            retention_limit: MAX_OPERATION_COUNT,
            retention_remaining: MAX_OPERATION_COUNT - retained_operation_directories,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    pub(super) async fn load(&self, plan_digest: &str) -> UseResult<Option<StateRestoreOperation>> {
        let directory = self.operation_directory(plan_digest)?;
        let journal = directory.join("operation.json");
        let metadata = match fs::symlink_metadata(&directory).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(operation_io("inspect restore operation", &directory, error)),
        };
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
            return Err(operation_invalid(
                "The whole-installation restore operation path is not an owned directory.",
            ));
        }
        validate_directory_chain(&self.state_root, &directory).await?;
        recover_temporary_json(&journal, MAX_OPERATION_BYTES).await?;
        let operation: Option<StateRestoreOperation> =
            read_optional_json(&journal, MAX_OPERATION_BYTES, "state restore operation").await?;
        operation
            .map(|operation| {
                operation.validate()?;
                if operation.plan_digest != plan_digest {
                    return Err(operation_invalid(
                        "The whole-installation restore operation does not match its owned path.",
                    ));
                }
                Ok(operation)
            })
            .transpose()
    }

    pub(super) async fn nonterminal(&self) -> UseResult<Option<StateRestoreOperation>> {
        let operations = history::load_for_mutation(self).await?;
        let mut nonterminal = None;
        for operation in operations {
            if operation.status != StateRestoreOperationStatus::Completed {
                if nonterminal.is_some() {
                    return Err(operation_invalid(
                        "Multiple nonterminal whole-installation restores are retained.",
                    ));
                }
                nonterminal = Some(operation);
            }
        }
        Ok(nonterminal)
    }

    pub(super) async fn begin(&self, operation: &StateRestoreOperation) -> UseResult<()> {
        operation.validate()?;
        if let Some(current) = self.load(&operation.plan_digest).await? {
            if current == *operation {
                return Ok(());
            }
            return Err(operation_conflict(
                "The reviewed whole-installation restore already has different durable evidence.",
            ));
        }
        let directory = self
            .ensure_operation_directory(&operation.plan_digest)
            .await?;
        write_json(
            &directory.join("operation.json"),
            operation,
            MAX_OPERATION_BYTES,
        )
        .await
    }

    pub(super) async fn save(&self, operation: &StateRestoreOperation) -> UseResult<()> {
        operation.validate()?;
        let current = self.load(&operation.plan_digest).await?.ok_or_else(|| {
            operation_conflict("The whole-installation restore journal is missing.")
        })?;
        if current.plan != operation.plan
            || current.plan_digest != operation.plan_digest
            || current.rollback_backup_manifest_digest != operation.rollback_backup_manifest_digest
            || current.started_at_ms != operation.started_at_ms
            || operation.status.sequence() != current.status.sequence().saturating_add(1)
        {
            return Err(operation_conflict(
                "The whole-installation restore journal cannot advance from its durable state.",
            ));
        }
        let journal = self
            .operation_directory(&operation.plan_digest)?
            .join("operation.json");
        write_json(&journal, operation, MAX_OPERATION_BYTES).await
    }

    pub(super) async fn activate(&self, operation: &StateRestoreOperation) -> UseResult<()> {
        operation.validate()?;
        let expected = ActiveStateRestoreMarker::new(operation)?;
        if let Some(current) = self.active().await? {
            if current.binds_operation(operation)? {
                return Ok(());
            }
            return Err(operation_conflict(
                "Another durable whole-installation restore is already active.",
            ));
        }
        write_json(
            &self.state_root.join(ACTIVE_STATE_RESTORE_MARKER),
            &expected,
            MAX_MARKER_BYTES,
        )
        .await
    }

    pub(super) async fn clear_active(&self, operation: &StateRestoreOperation) -> UseResult<bool> {
        let Some(current) = self.active().await? else {
            return Ok(false);
        };
        if !current.binds_operation(operation)? {
            return Err(operation_conflict(
                "The active state restore marker belongs to another operation.",
            ));
        }
        let path = self.state_root.join(ACTIVE_STATE_RESTORE_MARKER);
        fs::remove_file(&path)
            .await
            .map_err(|error| operation_io("remove active restore marker", &path, error))?;
        sync_directory(&self.state_root).await?;
        Ok(true)
    }

    fn operation_directory(&self, plan_digest: &str) -> UseResult<PathBuf> {
        let digest = plan_digest.strip_prefix("sha256:").filter(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        });
        let digest = digest.ok_or_else(|| {
            operation_invalid("The whole-installation restore plan digest is invalid.")
        })?;
        Ok(self.root.join(digest))
    }

    async fn ensure_operation_directory(&self, plan_digest: &str) -> UseResult<PathBuf> {
        ensure_owned_directory(&self.state_root).await?;
        ensure_owned_directory(&self.state_root.join("operations")).await?;
        ensure_owned_directory(&self.root).await?;
        history::reserve(self, plan_digest).await?;
        let directory = self.operation_directory(plan_digest)?;
        ensure_owned_directory(&directory).await?;
        Ok(directory)
    }
}

fn operation_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.state_restore_operation_invalid", message)
}

fn operation_conflict(message: impl Into<String>) -> UseError {
    UseError::new("use.state_restore_operation_conflict", message)
}

fn operation_io(action: &str, path: &Path, error: io::Error) -> UseError {
    UseError::new(
        "use.state_restore_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}
