use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{PlanScope, UseError, UseResult};
use serde::{Deserialize, Serialize};
use tokio::fs;

use super::{
    valid_sha256, OkfKnowledgeDatabaseEvidence, OkfKnowledgeFileEvidence, OkfKnowledgeRestorePlan,
};
use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;

mod inventory;

pub const OKF_KNOWLEDGE_RESTORE_OPERATION_SCHEMA: &str =
    "a3s.use.okf-knowledge-restore-operation.v2";
pub const OKF_KNOWLEDGE_RESTORE_RESULT_SCHEMA: &str = "a3s.use.okf-knowledge-restore-result.v2";

const ACTIVE_RESTORE_SCHEMA: &str = "a3s.use.active-state-restore.v2";
const MAX_RESTORE_OPERATION_BYTES: u64 = 512 * 1024;
const MAX_ACTIVE_RESTORE_BYTES: u64 = 640 * 1024;
pub(super) const MAX_RESTORE_OPERATIONS_PER_SCOPE: usize = 32;
pub(super) const MAX_RESTORE_FILE_BYTES: u64 = 64 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RestoreOperationStatus {
    Planned,
    Staged,
    BindingsRestored,
    PriorMoved,
    Published,
    Completed,
}

impl RestoreOperationStatus {
    pub(super) const fn sequence(self) -> u8 {
        match self {
            Self::Planned => 0,
            Self::Staged => 1,
            Self::BindingsRestored => 2,
            Self::PriorMoved => 3,
            Self::Published => 4,
            Self::Completed => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreFileEvidence {
    pub(super) bytes: u64,
    pub(super) sha256: String,
}

impl RestoreFileEvidence {
    fn validate_database(&self) -> bool {
        self.bytes > 0 && self.validate_sidecar()
    }

    fn validate_sidecar(&self) -> bool {
        self.bytes <= MAX_RESTORE_FILE_BYTES && valid_sha256(&self.sha256)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestorePriorFiles {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) database: Option<RestoreFileEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) wal: Option<RestoreFileEvidence>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) shm: Option<RestoreFileEvidence>,
}

impl RestorePriorFiles {
    pub(super) fn preserved_count(&self) -> usize {
        usize::from(self.database.is_some())
            + usize::from(self.wal.is_some())
            + usize::from(self.shm.is_some())
    }

    fn validate_for(&self, plan: &OkfKnowledgeRestorePlan) -> UseResult<()> {
        if self
            .database
            .as_ref()
            .is_some_and(|value| !value.validate_database())
            || self
                .wal
                .as_ref()
                .is_some_and(|value| !value.validate_sidecar())
            || self
                .shm
                .as_ref()
                .is_some_and(|value| !value.validate_sidecar())
            || self.database.is_none() && (self.wal.is_some() || self.shm.is_some())
        {
            return Err(operation_invalid(
                "The Knowledge restore prior-file inventory is invalid or exceeds its bounds.",
            ));
        }
        let matches = match (&plan.database_before, &self.database) {
            (None, None) => self.wal.is_none() && self.shm.is_none(),
            (Some(before), Some(database)) => {
                before.bytes == database.bytes
                    && before.sha256 == database.sha256
                    && same_optional_evidence(before.wal.as_ref(), self.wal.as_ref())
                    && same_optional_evidence(before.shm.as_ref(), self.shm.as_ref())
            }
            _ => false,
        };
        if !matches {
            return Err(operation_invalid(
                "The Knowledge restore prior database does not match the reviewed plan.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreOperation {
    pub(super) schema: String,
    pub(super) plan: OkfKnowledgeRestorePlan,
    pub(super) plan_digest: String,
    pub(super) status: RestoreOperationStatus,
    pub(super) prior_files: RestorePriorFiles,
    pub(super) started_at_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) completed_at_ms: Option<u64>,
}

impl RestoreOperation {
    pub(super) fn new(
        plan: OkfKnowledgeRestorePlan,
        plan_digest: String,
        prior_files: RestorePriorFiles,
        started_at_ms: u64,
    ) -> UseResult<Self> {
        let operation = Self {
            schema: OKF_KNOWLEDGE_RESTORE_OPERATION_SCHEMA.to_owned(),
            plan,
            plan_digest,
            status: RestoreOperationStatus::Planned,
            prior_files,
            started_at_ms,
            completed_at_ms: None,
        };
        operation.validate()?;
        Ok(operation)
    }

    pub(super) fn validate(&self) -> UseResult<()> {
        self.plan.validate()?;
        self.prior_files.validate_for(&self.plan)?;
        if self.schema != OKF_KNOWLEDGE_RESTORE_OPERATION_SCHEMA
            || self.plan.status != super::OkfKnowledgeRestorePlanStatus::Required
            || !valid_sha256(&self.plan_digest)
            || self.plan.descriptor_digest()? != self.plan_digest
            || self.started_at_ms == 0
        {
            return Err(operation_invalid(
                "The Knowledge restore operation identity is invalid.",
            ));
        }
        match (self.status, self.completed_at_ms) {
            (RestoreOperationStatus::Completed, Some(completed_at_ms))
                if completed_at_ms >= self.started_at_ms =>
            {
                Ok(())
            }
            (RestoreOperationStatus::Completed, _) => Err(operation_invalid(
                "A completed Knowledge restore has no valid completion time.",
            )),
            (_, None) => Ok(()),
            (_, Some(_)) => Err(operation_invalid(
                "A nonterminal Knowledge restore carries a completion time.",
            )),
        }
    }

    pub(super) fn advance(
        &mut self,
        next: RestoreOperationStatus,
        completed_at_ms: Option<u64>,
    ) -> UseResult<bool> {
        self.validate()?;
        if self.status == next {
            return Ok(false);
        }
        if next.sequence() != self.status.sequence().saturating_add(1) {
            return Err(operation_conflict(
                "Knowledge restore checkpoints must advance in canonical order.",
            ));
        }
        let mut candidate = self.clone();
        candidate.status = next;
        candidate.completed_at_ms = completed_at_ms;
        candidate.validate()?;
        *self = candidate;
        Ok(true)
    }

    pub(super) fn result(&self) -> UseResult<OkfKnowledgeRestoreResult> {
        self.validate()?;
        if self.status != RestoreOperationStatus::Completed {
            return Err(operation_conflict(
                "A nonterminal Knowledge restore has no final result.",
            ));
        }
        let result = OkfKnowledgeRestoreResult {
            schema: OKF_KNOWLEDGE_RESTORE_RESULT_SCHEMA.to_owned(),
            scope: self.plan.scope.clone(),
            changed: true,
            plan_digest: self.plan_digest.clone(),
            database_before: self.plan.database_before.clone(),
            database_after: OkfKnowledgeDatabaseEvidence {
                bytes: self.plan.backup.database_bytes,
                sha256: self.plan.backup.database_sha256.clone(),
                integrity_verified: true,
                wal: None,
                shm: None,
            },
            preserved_prior_files: self.prior_files.preserved_count(),
            restored_bindings: self.plan.missing_bindings,
            completed_at_ms: self.completed_at_ms,
        };
        result.validate()?;
        Ok(result)
    }

    fn initial(&self) -> UseResult<Self> {
        self.validate()?;
        let mut initial = self.clone();
        initial.status = RestoreOperationStatus::Planned;
        initial.completed_at_ms = None;
        initial.validate()?;
        Ok(initial)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OkfKnowledgeRestoreResult {
    pub schema: String,
    pub scope: PlanScope,
    pub changed: bool,
    pub plan_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub database_before: Option<OkfKnowledgeDatabaseEvidence>,
    pub database_after: OkfKnowledgeDatabaseEvidence,
    pub preserved_prior_files: usize,
    pub restored_bindings: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub completed_at_ms: Option<u64>,
}

impl OkfKnowledgeRestoreResult {
    pub(super) fn no_change(
        plan: &OkfKnowledgeRestorePlan,
        plan_digest: String,
    ) -> UseResult<Self> {
        plan.validate()?;
        let database = plan.database_before.clone().ok_or_else(|| {
            operation_invalid("A no-change restore plan has no current database evidence.")
        })?;
        let result = Self {
            schema: OKF_KNOWLEDGE_RESTORE_RESULT_SCHEMA.to_owned(),
            scope: plan.scope.clone(),
            changed: false,
            plan_digest,
            database_before: Some(database.clone()),
            database_after: database,
            preserved_prior_files: 0,
            restored_bindings: 0,
            completed_at_ms: None,
        };
        result.validate()?;
        Ok(result)
    }

    pub fn validate(&self) -> UseResult<()> {
        let before_valid = self
            .database_before
            .as_ref()
            .is_none_or(OkfKnowledgeDatabaseEvidence::validate);
        if self.schema != OKF_KNOWLEDGE_RESTORE_RESULT_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || !before_valid
            || !self.database_after.validate()
            || !self.database_after.integrity_verified
            || self.preserved_prior_files > 3
            || self.restored_bindings > crate::okf_knowledge::MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS
        {
            return Err(operation_invalid(
                "The Knowledge restore result is internally inconsistent.",
            ));
        }
        match (self.changed, self.completed_at_ms) {
            (true, Some(completed_at_ms))
                if completed_at_ms > 0
                    && self.database_after.wal.is_none()
                    && self.database_after.shm.is_none() =>
            {
                Ok(())
            }
            (false, None)
                if self.database_before.as_ref() == Some(&self.database_after)
                    && self.preserved_prior_files == 0
                    && self.restored_bindings == 0 =>
            {
                Ok(())
            }
            _ => Err(operation_invalid(
                "The Knowledge restore result does not match its terminal outcome.",
            )),
        }
    }
}

fn same_optional_evidence(
    expected: Option<&OkfKnowledgeFileEvidence>,
    actual: Option<&RestoreFileEvidence>,
) -> bool {
    match (expected, actual) {
        (None, None) => true,
        (Some(expected), Some(actual)) => {
            expected.bytes == actual.bytes && expected.sha256 == actual.sha256
        }
        _ => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ActiveRestoreMarker {
    schema: String,
    pub(super) scope: PlanScope,
    pub(super) plan_digest: String,
    pub(super) operation: RestoreOperation,
}

impl ActiveRestoreMarker {
    fn new(operation: &RestoreOperation) -> UseResult<Self> {
        let operation = operation.initial()?;
        let marker = Self {
            schema: ACTIVE_RESTORE_SCHEMA.to_owned(),
            scope: operation.plan.scope.clone(),
            plan_digest: operation.plan_digest.clone(),
            operation,
        };
        marker.validate()?;
        Ok(marker)
    }

    fn validate(&self) -> UseResult<()> {
        self.operation.validate()?;
        if self.schema != ACTIVE_RESTORE_SCHEMA
            || !valid_sha256(&self.plan_digest)
            || self.operation.status != RestoreOperationStatus::Planned
            || self.scope != self.operation.plan.scope
            || self.plan_digest != self.operation.plan_digest
        {
            return Err(operation_invalid(
                "The active state restore marker is invalid.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub(super) struct RestoreOperationPaths {
    pub(super) directory: PathBuf,
    pub(super) journal: PathBuf,
    pub(super) candidate: PathBuf,
    pub(super) prior_database: PathBuf,
    pub(super) prior_wal: PathBuf,
    pub(super) prior_shm: PathBuf,
}

#[derive(Debug, Clone)]
pub(super) struct RestoreOperationStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl RestoreOperationStore {
    pub(super) fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("knowledge").join("restores"),
            state_root,
        }
    }

    pub(super) async fn active(&self) -> UseResult<Option<ActiveRestoreMarker>> {
        read_optional_json(
            &self.state_root.join(ACTIVE_STATE_RESTORE_MARKER),
            MAX_ACTIVE_RESTORE_BYTES,
            "active Knowledge restore marker",
        )
        .await?
        .map(|marker: ActiveRestoreMarker| {
            marker.validate()?;
            Ok(marker)
        })
        .transpose()
    }

    pub(super) async fn load(
        &self,
        scope: &PlanScope,
        plan_digest: &str,
    ) -> UseResult<Option<RestoreOperation>> {
        let paths = self.paths(scope, plan_digest)?;
        let directory_metadata = match fs::symlink_metadata(&paths.directory).await {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(operation_io(
                    "inspect Knowledge restore operation directory",
                    &paths.directory,
                    error,
                ));
            }
        };
        if a3s_use_core::metadata_is_link_or_reparse_point(&directory_metadata)
            || !directory_metadata.is_dir()
        {
            return Err(operation_invalid(
                "The Knowledge restore operation path is not an owned directory.",
            ));
        }
        validate_existing_directory_chain(&self.state_root, &paths.directory).await?;
        let Some(operation) = read_optional_json(
            &paths.journal,
            MAX_RESTORE_OPERATION_BYTES,
            "Knowledge restore operation",
        )
        .await?
        else {
            return Ok(None);
        };
        let operation: RestoreOperation = operation;
        operation.validate()?;
        if operation.plan.scope != *scope || operation.plan_digest != plan_digest {
            return Err(operation_invalid(
                "The Knowledge restore operation does not match its owned path.",
            ));
        }
        Ok(Some(operation))
    }

    pub(super) async fn begin(&self, operation: &RestoreOperation) -> UseResult<()> {
        operation.validate()?;
        let paths = self.prepare(operation).await?;
        if let Some(current) = self
            .load(&operation.plan.scope, &operation.plan_digest)
            .await?
        {
            if current == *operation {
                return Ok(());
            }
            return Err(operation_conflict(
                "The reviewed Knowledge restore already has different durable operation evidence.",
            ));
        }
        remove_temporary_json(&paths.journal, MAX_RESTORE_OPERATION_BYTES).await?;
        ensure_empty_operation_directory(&paths).await?;
        write_json(&paths.journal, operation, MAX_RESTORE_OPERATION_BYTES).await
    }

    pub(super) async fn prepare(
        &self,
        operation: &RestoreOperation,
    ) -> UseResult<RestoreOperationPaths> {
        operation.validate()?;
        let paths = self
            .ensure_operation_directory(&operation.plan, &operation.plan_digest)
            .await?;
        if let Some(current) = self
            .load(&operation.plan.scope, &operation.plan_digest)
            .await?
        {
            if current.plan != operation.plan
                || current.plan_digest != operation.plan_digest
                || current.prior_files != operation.prior_files
                || current.started_at_ms != operation.started_at_ms
            {
                return Err(operation_conflict(
                    "The reviewed Knowledge restore already has different durable operation evidence.",
                ));
            }
            return Ok(paths);
        }
        remove_temporary_json(&paths.journal, MAX_RESTORE_OPERATION_BYTES).await?;
        ensure_empty_operation_directory(&paths).await?;
        Ok(paths)
    }

    pub(super) async fn save(&self, operation: &RestoreOperation) -> UseResult<()> {
        operation.validate()?;
        let paths = self.paths(&operation.plan.scope, &operation.plan_digest)?;
        validate_existing_directory_chain(&self.state_root, &paths.directory).await?;
        write_json(&paths.journal, operation, MAX_RESTORE_OPERATION_BYTES).await
    }

    pub(super) async fn activate(&self, operation: &RestoreOperation) -> UseResult<()> {
        operation.validate()?;
        let expected = ActiveRestoreMarker::new(operation)?;
        if let Some(current) = self.active().await? {
            if current == expected {
                return Ok(());
            }
            return Err(operation_conflict(
                "Another durable state restore is already active.",
            ));
        }
        write_json(
            &self.state_root.join(ACTIVE_STATE_RESTORE_MARKER),
            &expected,
            MAX_ACTIVE_RESTORE_BYTES,
        )
        .await
    }

    pub(super) async fn clear_active(&self, operation: &RestoreOperation) -> UseResult<bool> {
        operation.validate()?;
        let Some(current) = self.active().await? else {
            return Ok(false);
        };
        if current.scope != operation.plan.scope || current.plan_digest != operation.plan_digest {
            return Err(operation_conflict(
                "The active state restore marker belongs to another operation.",
            ));
        }
        let path = self.state_root.join(ACTIVE_STATE_RESTORE_MARKER);
        fs::remove_file(&path).await.map_err(|error| {
            operation_io("remove active Knowledge restore marker", &path, error)
        })?;
        sync_directory(&self.state_root).await?;
        Ok(true)
    }

    pub(super) fn paths(
        &self,
        scope: &PlanScope,
        plan_digest: &str,
    ) -> UseResult<RestoreOperationPaths> {
        let digest = digest_segment(plan_digest)?;
        let directory = self.scope_directory(scope)?.join(digest);
        Ok(RestoreOperationPaths {
            journal: directory.join("operation.json"),
            candidate: directory.join("candidate.sqlite3"),
            prior_database: directory.join("prior.sqlite3"),
            prior_wal: directory.join("prior.sqlite3-wal"),
            prior_shm: directory.join("prior.sqlite3-shm"),
            directory,
        })
    }

    fn scope_directory(&self, scope: &PlanScope) -> UseResult<PathBuf> {
        let scope_digest = scope.storage_key().map_err(|_| {
            operation_invalid("The Knowledge restore installation identity is invalid.")
        })?;
        Ok(self.root.join(scope.kind.as_str()).join(scope_digest))
    }

    async fn ensure_operation_directory(
        &self,
        plan: &OkfKnowledgeRestorePlan,
        plan_digest: &str,
    ) -> UseResult<RestoreOperationPaths> {
        plan.validate()?;
        let paths = self.paths(&plan.scope, plan_digest)?;
        let scope_directory = paths.directory.parent().ok_or_else(|| {
            operation_invalid("The Knowledge restore operation path has no scope directory.")
        })?;
        ensure_owned_directory_chain(&self.state_root, scope_directory).await?;
        enforce_operation_bound(scope_directory, paths.directory.file_name()).await?;
        ensure_owned_directory_chain(&self.state_root, &paths.directory).await?;
        Ok(paths)
    }
}

async fn enforce_operation_bound(
    scope_directory: &Path,
    requested: Option<&std::ffi::OsStr>,
) -> UseResult<()> {
    let requested = requested.and_then(|value| value.to_str());
    let mut entries = fs::read_dir(scope_directory)
        .await
        .map_err(|error| operation_io("read Knowledge restore scope", scope_directory, error))?;
    let mut count = 0_usize;
    let mut requested_exists = false;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| operation_io("read Knowledge restore operation", scope_directory, error))?
    {
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| operation_invalid("A Knowledge restore operation name is invalid."))?;
        let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
            operation_io("inspect Knowledge restore operation", &entry.path(), error)
        })?;
        if !valid_digest_segment(&name)
            || a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || !metadata.is_dir()
        {
            return Err(operation_invalid(
                "The Knowledge restore operation layout contains an unowned entry.",
            ));
        }
        count = count.saturating_add(1);
        requested_exists |= requested == Some(name.as_str());
        if count > MAX_RESTORE_OPERATIONS_PER_SCOPE {
            return Err(operation_limit());
        }
    }
    if !requested_exists && count >= MAX_RESTORE_OPERATIONS_PER_SCOPE {
        return Err(operation_limit());
    }
    Ok(())
}

async fn ensure_empty_operation_directory(paths: &RestoreOperationPaths) -> UseResult<()> {
    let mut entries = fs::read_dir(&paths.directory).await.map_err(|error| {
        operation_io(
            "read new Knowledge restore operation directory",
            &paths.directory,
            error,
        )
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| {
            operation_io(
                "read new Knowledge restore operation entry",
                &paths.directory,
                error,
            )
        })?
        .is_some()
    {
        return Err(operation_invalid(
            "A new Knowledge restore operation directory is not empty.",
        ));
    }
    Ok(())
}

async fn read_optional_json<T: serde::de::DeserializeOwned>(
    path: &Path,
    maximum: u64,
    label: &str,
) -> UseResult<Option<T>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(operation_io(&format!("inspect {label}"), path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > maximum
    {
        return Err(operation_invalid(format!(
            "The {label} is not a bounded owned regular file."
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| operation_io(&format!("read {label}"), path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > maximum {
        return Err(operation_invalid(format!(
            "The {label} changed outside its size bound while reading."
        )));
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|error| operation_invalid(format!("The {label} is invalid JSON: {error}")))
}

async fn write_json<T: Serialize>(path: &Path, value: &T, maximum: u64) -> UseResult<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|error| {
        operation_invalid(format!(
            "Failed to encode Knowledge restore evidence: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > maximum {
        return Err(operation_invalid(
            "The encoded Knowledge restore evidence exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(|| {
        operation_invalid("The Knowledge restore evidence path has no parent directory.")
    })?;
    remove_temporary_json(path, maximum).await?;
    let mut temporary_name = path
        .file_name()
        .ok_or_else(|| operation_invalid("The Knowledge restore evidence path has no file name."))?
        .to_os_string();
    temporary_name.push(".tmp");
    let temporary = parent.join(temporary_name);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options.open(&temporary).await.map_err(|error| {
        operation_io(
            "create temporary Knowledge restore evidence",
            &temporary,
            error,
        )
    })?;
    use tokio::io::AsyncWriteExt;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(operation_io(
            "write temporary Knowledge restore evidence",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(operation_io(
            "sync temporary Knowledge restore evidence",
            &temporary,
            error,
        ));
    }
    drop(file);
    let target = path.to_path_buf();
    let error_target = target.clone();
    let activation = tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_replace_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        operation_backend(format!(
            "Failed to activate Knowledge restore evidence '{}': blocking task failed: {error}",
            error_target.display()
        ))
    })?;
    if let Err(error) = activation {
        return Err(operation_io(
            "activate Knowledge restore evidence",
            &error_target,
            error,
        ));
    }
    sync_directory(parent).await
}

async fn remove_temporary_json(path: &Path, maximum: u64) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        operation_invalid("The Knowledge restore evidence path has no parent directory.")
    })?;
    let mut temporary_name = path
        .file_name()
        .ok_or_else(|| operation_invalid("The Knowledge restore evidence path has no file name."))?
        .to_os_string();
    temporary_name.push(".tmp");
    let temporary = parent.join(temporary_name);
    let metadata = match fs::symlink_metadata(&temporary).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(operation_io(
                "inspect temporary Knowledge restore evidence",
                &temporary,
                error,
            ));
        }
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > maximum
    {
        return Err(operation_invalid(
            "Temporary Knowledge restore evidence is not a bounded owned regular file.",
        ));
    }
    fs::remove_file(&temporary).await.map_err(|error| {
        operation_io(
            "remove interrupted Knowledge restore evidence",
            &temporary,
            error,
        )
    })?;
    sync_directory(parent).await
}

async fn ensure_owned_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(operation_invalid(
            "The Knowledge restore directory escapes the configured state root.",
        ));
    }
    validate_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| operation_invalid("The Knowledge restore directory is not state-owned."))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(operation_io(
                    "create Knowledge restore directory",
                    &current,
                    error,
                ));
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(operation_invalid(
            "The Knowledge restore directory is not state-owned.",
        ));
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| operation_invalid("The Knowledge restore directory is not state-owned."))?;
    let mut current = root.to_path_buf();
    for component in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(component) = component {
            current.push(component.as_os_str());
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| operation_io("inspect Knowledge restore directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(operation_invalid(
            "A Knowledge restore directory is not an owned directory.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| operation_io("open Knowledge restore directory", path, error))?
        .sync_all()
        .await
        .map_err(|error| operation_io("sync Knowledge restore directory", path, error))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn digest_segment(value: &str) -> UseResult<&str> {
    value
        .strip_prefix("sha256:")
        .filter(|value| valid_digest_segment(value))
        .ok_or_else(|| operation_invalid("The Knowledge restore plan digest is invalid."))
}

fn valid_digest_segment(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn operation_limit() -> UseError {
    operation_error(
        "use.okf.knowledge_restore_retention_required",
        format!(
            "The Knowledge restore history reached its bounded limit of {MAX_RESTORE_OPERATIONS_PER_SCOPE} operations for this scope."
        ),
    )
    .with_suggestion(
        "Archive and remove a reviewed terminal restore directory according to the operator retention policy before starting another restore.",
    )
}

fn operation_invalid(message: impl Into<String>) -> UseError {
    operation_error("use.okf.knowledge_restore_operation_invalid", message)
}

fn operation_conflict(message: impl Into<String>) -> UseError {
    operation_error("use.okf.knowledge_restore_operation_conflict", message)
}

fn operation_backend(message: impl Into<String>) -> UseError {
    operation_error("use.okf.knowledge_restore_backend_failed", message)
}

fn operation_io(action: &str, path: &Path, error: io::Error) -> UseError {
    operation_error(
        "use.okf.knowledge_restore_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn operation_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
