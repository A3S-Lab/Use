use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{OkfKnowledgeObservation, OkfProjectionReceipt, PlanScope, UseError, UseResult};
use a3s_use_extension::ExtensionPaths;
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use super::{
    OkfKnowledgeAdapter, OkfKnowledgeBinding, OkfKnowledgeSearchRequest,
    OkfKnowledgeSearchResponse, OkfKnowledgeStageRequest,
};

mod audit;
mod backup;
mod filesystem;
mod index;
mod policy;
mod projection;
mod record;
mod schema;
mod search;
mod storage;

pub use audit::{
    OkfKnowledgeIntegrityReport, OkfKnowledgeSearchIndexRepair,
    OKF_KNOWLEDGE_INTEGRITY_REPORT_SCHEMA, OKF_KNOWLEDGE_SEARCH_INDEX_REPAIR_SCHEMA,
};
pub use backup::{OkfKnowledgeBackupManifest, OKF_KNOWLEDGE_BACKUP_SCHEMA};
use filesystem::{prepare_scope_database, LockMode};
pub use policy::{
    OkfKnowledgeStoragePolicy, DEFAULT_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES,
    DEFAULT_OKF_KNOWLEDGE_SCOPE_PROJECTIONS, DEFAULT_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
    MAX_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES, MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS,
    MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
};
pub use storage::OkfKnowledgeStorageUsage;

/// Cross-platform production Knowledge backend for standalone A3S Use.
///
/// Each complete User/Workspace scope owns a separate SQLite/FTS5 database.
/// Stage, promotion, selection, and receipt-owned removal are transactional;
/// search accepts only exact retained projections supplied by a reviewed
/// capability snapshot or an already-open session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteOkfKnowledgeAdapter {
    state_root: PathBuf,
    root: PathBuf,
    policy: OkfKnowledgeStoragePolicy,
}

impl SqliteOkfKnowledgeAdapter {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        Self::with_policy(state_root, OkfKnowledgeStoragePolicy::default())
    }

    pub fn with_policy(state_root: impl Into<PathBuf>, policy: OkfKnowledgeStoragePolicy) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("knowledge").join("sqlite"),
            state_root,
            policy,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::new(paths.state_root())
    }

    pub fn from_extension_paths_with_policy(
        paths: &ExtensionPaths,
        policy: OkfKnowledgeStoragePolicy,
    ) -> Self {
        Self::with_policy(paths.state_root(), policy)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub const fn policy(&self) -> &OkfKnowledgeStoragePolicy {
        &self.policy
    }

    pub async fn usage(&self, scope: &PlanScope) -> UseResult<OkfKnowledgeStorageUsage> {
        let guard = self.database_guard(scope, LockMode::Shared).await?;
        let exists = tokio::fs::try_exists(&guard.path).await.map_err(|error| {
            UseError::new(
                "use.okf.knowledge_database_io",
                format!(
                    "Failed to inspect Knowledge database '{}': {error}",
                    guard.path.display()
                ),
            )
        })?;
        if !exists {
            return Ok(OkfKnowledgeStorageUsage::empty(scope.clone(), &self.policy));
        }
        let scope = scope.clone();
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            let connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open accounted Knowledge database", error))?;
            storage::usage(&connection, &scope, &policy)
        })
        .await
        .map_err(|error| blocking_error("account for OKF Knowledge storage", error))?
    }

    /// Validate SQLite, receipt, scope, foreign-key, and derived FTS evidence.
    pub async fn audit(&self, scope: &PlanScope) -> UseResult<OkfKnowledgeIntegrityReport> {
        let guard = self.database_guard(scope, LockMode::Shared).await?;
        require_database(&guard.path).await?;
        let scope = scope.clone();
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            let connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open audited Knowledge database", error))?;
            audit::audit(&connection, &scope, &policy)
        })
        .await
        .map_err(|error| blocking_error("audit OKF Knowledge storage", error))?
    }

    /// Rebuild only the FTS5 rows derived from validated retained documents.
    pub async fn repair_search_index(
        &self,
        scope: &PlanScope,
    ) -> UseResult<OkfKnowledgeSearchIndexRepair> {
        let guard = self.database_guard(scope, LockMode::Exclusive).await?;
        require_database(&guard.path).await?;
        let scope = scope.clone();
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            let mut connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open repairable Knowledge database", error))?;
            audit::repair_search_index(&mut connection, &scope, &policy)
        })
        .await
        .map_err(|error| blocking_error("repair the OKF Knowledge search index", error))?
    }

    /// Write a consistent, digest-bound snapshot without overwriting a file.
    pub async fn backup(
        &self,
        scope: &PlanScope,
        destination: impl Into<PathBuf>,
    ) -> UseResult<OkfKnowledgeBackupManifest> {
        let destination = destination.into();
        let guard = self.database_guard(scope, LockMode::Exclusive).await?;
        require_database(&guard.path).await?;
        let scope = scope.clone();
        let policy = self.policy;
        let created_at_ms = now_ms()?;
        tokio::task::spawn_blocking(move || {
            backup::create(&guard.path, &scope, &policy, &destination, created_at_ms)
        })
        .await
        .map_err(|error| blocking_error("back up OKF Knowledge storage", error))?
    }

    /// Verify an offline backup without changing live Knowledge state.
    pub async fn verify_backup(
        backup_path: impl Into<PathBuf>,
        expected_scope: Option<&PlanScope>,
    ) -> UseResult<OkfKnowledgeBackupManifest> {
        let backup_path = backup_path.into();
        let expected_scope = expected_scope.cloned();
        tokio::task::spawn_blocking(move || backup::verify(&backup_path, expected_scope.as_ref()))
            .await
            .map_err(|error| blocking_error("verify an OKF Knowledge backup", error))?
    }

    fn scope_directory(&self, scope: &PlanScope) -> UseResult<PathBuf> {
        if !valid_machine_id(&scope.id) {
            return Err(UseError::new(
                "use.okf.knowledge_database_scope_invalid",
                "The Knowledge database requires a valid complete User or Workspace scope.",
            ));
        }
        let digest = format!("{:x}", Sha256::digest(scope.id.as_bytes()));
        Ok(self.root.join(scope.kind.as_str()).join(digest))
    }

    async fn database_guard(
        &self,
        scope: &PlanScope,
        mode: LockMode,
    ) -> UseResult<filesystem::ScopeDatabaseGuard> {
        let directory = self.scope_directory(scope)?;
        prepare_scope_database(&self.state_root, &self.root, &directory, mode).await
    }
}

async fn require_database(path: &Path) -> UseResult<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_file() => Ok(()),
        Ok(_) => Err(UseError::new(
            "use.okf.knowledge_database_path_invalid",
            "The Knowledge database path is not an owned regular file.",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(UseError::new(
            "use.okf.knowledge_database_missing",
            "The complete User or Workspace scope has no Knowledge database to operate on.",
        )),
        Err(error) => Err(UseError::new(
            "use.okf.knowledge_database_io",
            format!(
                "Failed to inspect Knowledge database '{}': {error}",
                path.display()
            ),
        )),
    }
}

#[async_trait]
impl OkfKnowledgeAdapter for SqliteOkfKnowledgeAdapter {
    async fn stage(&self, request: &OkfKnowledgeStageRequest) -> UseResult<OkfKnowledgeBinding> {
        let spec = request.spec().clone();
        let files = request.shared_files();
        let prepared = tokio::task::spawn_blocking({
            let spec = spec.clone();
            move || index::prepare(spec, files)
        })
        .await
        .map_err(|error| blocking_error("build the OKF cited-search index", error))??;
        let guard = self
            .database_guard(&spec.scope, LockMode::Exclusive)
            .await?;
        let now_ms = now_ms()?;
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            let mut connection = schema::open(&guard.path, true)
                .map_err(|error| sqlite_error("open staged Knowledge database", error))?;
            record::stage(&mut connection, &spec, &prepared, now_ms, &policy)
        })
        .await
        .map_err(|error| blocking_error("stage the OKF Knowledge index", error))?
    }

    async fn promote(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        let receipt = receipt.clone();
        let guard = self
            .database_guard(&receipt.scope, LockMode::Exclusive)
            .await?;
        let now_ms = now_ms()?;
        tokio::task::spawn_blocking(move || {
            let mut connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open promoted Knowledge database", error))?;
            record::promote(&mut connection, &receipt, now_ms)
        })
        .await
        .map_err(|error| blocking_error("promote the OKF Knowledge index", error))?
    }

    async fn observe(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        let receipt = receipt.clone();
        let guard = self
            .database_guard(&receipt.scope, LockMode::Shared)
            .await?;
        tokio::task::spawn_blocking(move || {
            let connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open observed Knowledge database", error))?;
            record::observe(&connection, &receipt)
        })
        .await
        .map_err(|error| blocking_error("observe the OKF Knowledge index", error))?
    }

    async fn remove(&self, receipt: &OkfProjectionReceipt) -> UseResult<OkfKnowledgeObservation> {
        let receipt = receipt.clone();
        let guard = self
            .database_guard(&receipt.scope, LockMode::Exclusive)
            .await?;
        let now_ms = now_ms()?;
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            let mut connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open removable Knowledge database", error))?;
            let removed = record::remove(&mut connection, &receipt, now_ms)?;
            storage::collect_garbage(&mut connection, &policy)?;
            Ok(removed)
        })
        .await
        .map_err(|error| blocking_error("remove the receipt-owned OKF Knowledge index", error))?
    }

    async fn search(
        &self,
        request: &OkfKnowledgeSearchRequest,
    ) -> UseResult<OkfKnowledgeSearchResponse> {
        request.validate()?;
        let request = request.clone();
        let guard = self
            .database_guard(&request.scope, LockMode::Shared)
            .await?;
        tokio::task::spawn_blocking(move || {
            let connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open cited-search Knowledge database", error))?;
            search::search(&connection, &request)
        })
        .await
        .map_err(|error| blocking_error("query the OKF Knowledge index", error))?
    }
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

fn now_ms() -> UseResult<u64> {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            UseError::new(
                "use.okf.knowledge_clock_invalid",
                format!("The system clock is before the Unix epoch: {error}"),
            )
        })?
        .as_millis();
    let millis = u64::try_from(millis).map_err(|_| {
        UseError::new(
            "use.okf.knowledge_clock_invalid",
            "The system clock exceeds the Knowledge timestamp range.",
        )
    })?;
    (millis > 0).then_some(millis).ok_or_else(|| {
        UseError::new(
            "use.okf.knowledge_clock_invalid",
            "The Knowledge timestamp must be non-zero.",
        )
    })
}

fn sqlite_error(action: &str, error: rusqlite::Error) -> UseError {
    UseError::new(
        "use.okf.knowledge_database_io",
        format!("Failed to {action}: {error}"),
    )
}

fn blocking_error(action: &str, error: tokio::task::JoinError) -> UseError {
    UseError::new(
        "use.okf.knowledge_backend_failed",
        format!("Failed to {action}: blocking task failed: {error}"),
    )
}

#[cfg(test)]
mod tests;
