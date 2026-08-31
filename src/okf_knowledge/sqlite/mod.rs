use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{
    InstallationId, OkfKnowledgeObservation, OkfProjectionReceipt, PlanScope, UseError, UseResult,
};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceGuard};
use async_trait::async_trait;

use super::{
    OkfKnowledgeAdapter, OkfKnowledgeBinding, OkfKnowledgeReadRequest, OkfKnowledgeReadResponse,
    OkfKnowledgeSearchRequest, OkfKnowledgeSearchResponse, OkfKnowledgeStageRequest,
};

mod audit;
mod backup;
mod backup_retention;
mod filesystem;
mod index;
mod policy;
mod projection;
mod read;
mod record;
mod schema;
mod search;
mod storage;

pub use audit::{
    OkfKnowledgeIntegrityReport, OkfKnowledgeSearchIndexRepair,
    OKF_KNOWLEDGE_INTEGRITY_REPORT_SCHEMA, OKF_KNOWLEDGE_SEARCH_INDEX_REPAIR_SCHEMA,
};
pub use backup::{OkfKnowledgeBackupManifest, OKF_KNOWLEDGE_BACKUP_SCHEMA};
pub(crate) use backup::{
    OkfKnowledgeRestoreInventory, VerifiedOkfKnowledgeBackup, MAX_BACKUP_ARCHIVE_BYTES,
    MAX_BACKUP_DATABASE_BYTES,
};
pub use backup_retention::{
    OkfKnowledgeBackupRetentionEntry, OkfKnowledgeBackupRetentionPlan,
    OkfKnowledgeBackupRetentionPolicy, OkfKnowledgeBackupRetentionResult,
    DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BACKUPS,
    DEFAULT_OKF_KNOWLEDGE_BACKUP_RETENTION_MAX_BYTES, MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BACKUPS,
    MAX_OKF_KNOWLEDGE_BACKUP_RETENTION_BYTES, OKF_KNOWLEDGE_BACKUP_RETENTION_PLAN_SCHEMA,
    OKF_KNOWLEDGE_BACKUP_RETENTION_RESULT_SCHEMA,
};
pub(crate) use filesystem::ScopeDatabaseGuard;
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
/// Each adapter is bound to one complete User/Workspace installation and owns
/// its separate SQLite/FTS5 database.
/// Stage, promotion, selection, and receipt-owned removal are transactional;
/// search accepts only exact retained projections supplied by a reviewed
/// capability snapshot or an already-open session. Another installation is
/// rejected before a database directory, lock, or SQLite connection exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SqliteOkfKnowledgeAdapter {
    installation: InstallationId,
    state_root: PathBuf,
    root: PathBuf,
    policy: OkfKnowledgeStoragePolicy,
}

impl SqliteOkfKnowledgeAdapter {
    /// Construct an adapter over an already installation-scoped state root.
    pub fn new(state_root: impl Into<PathBuf>, installation: InstallationId) -> UseResult<Self> {
        Self::with_policy(
            state_root,
            installation,
            OkfKnowledgeStoragePolicy::default(),
        )
    }

    pub fn with_policy(
        state_root: impl Into<PathBuf>,
        installation: InstallationId,
        policy: OkfKnowledgeStoragePolicy,
    ) -> UseResult<Self> {
        installation.validate()?;
        Ok(Self::from_parts(state_root.into(), installation, policy))
    }

    fn from_parts(
        state_root: PathBuf,
        installation: InstallationId,
        policy: OkfKnowledgeStoragePolicy,
    ) -> Self {
        Self {
            installation,
            root: state_root.join("knowledge").join("sqlite"),
            state_root,
            policy,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::from_parts(
            paths.installation_state_root(),
            paths.installation().clone(),
            OkfKnowledgeStoragePolicy::default(),
        )
    }

    pub fn from_extension_paths_with_policy(
        paths: &ExtensionPaths,
        policy: OkfKnowledgeStoragePolicy,
    ) -> Self {
        Self::from_parts(
            paths.installation_state_root(),
            paths.installation().clone(),
            policy,
        )
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    pub const fn policy(&self) -> &OkfKnowledgeStoragePolicy {
        &self.policy
    }

    pub async fn usage(&self, scope: &PlanScope) -> UseResult<OkfKnowledgeStorageUsage> {
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
        let guard = self.database_guard(scope, LockMode::Exclusive).await?;
        require_database(&guard.path).await?;
        let scope = scope.clone();
        let policy = self.policy;
        let created_at_ms = now_ms()?;
        tokio::task::spawn_blocking(move || {
            backup::create(
                &guard.path,
                &scope,
                &policy,
                &destination,
                created_at_ms,
                MAX_BACKUP_ARCHIVE_BYTES,
            )
        })
        .await
        .map_err(|error| blocking_error("back up OKF Knowledge storage", error))?
    }

    /// Snapshot an existing database while an installation-wide exclusive
    /// maintenance guard is already held. An absent database is represented
    /// as `None` without creating Knowledge directories or lock files. The
    /// caller validates the exact temporary-snapshot inventory before the
    /// destination archive is written.
    pub(crate) async fn backup_if_present_under_maintenance<F>(
        &self,
        maintenance: &StateMaintenanceGuard,
        scope: &PlanScope,
        destination: impl Into<PathBuf>,
        created_at_ms: u64,
        max_archive_bytes: u64,
        validate_inventory: F,
    ) -> UseResult<Option<OkfKnowledgeBackupManifest>>
    where
        F: FnOnce(&OkfKnowledgeRestoreInventory) -> UseResult<()> + Send + 'static,
    {
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(UseError::new(
                "use.okf.knowledge_maintenance_mismatch",
                "The Knowledge snapshot requires the exclusive maintenance guard for its exact installation state root.",
            ));
        }
        self.installation.ensure_same(scope)?;
        let directory = self.scope_directory(scope)?;
        let destination = destination.into();
        let Some(database_path) = filesystem::existing_scope_database_under_maintenance(
            &self.state_root,
            &self.root,
            &directory,
        )
        .await?
        else {
            validate_inventory(&OkfKnowledgeRestoreInventory {
                bindings: Vec::new(),
                selected: Vec::new(),
            })?;
            tokio::task::spawn_blocking(move || backup::validate_new_destination(&destination))
                .await
                .map_err(|error| {
                    blocking_error("validate an absent OKF Knowledge backup destination", error)
                })??;
            return Ok(None);
        };
        let scope = scope.clone();
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            backup::create_validated(
                &database_path,
                &scope,
                &policy,
                &destination,
                created_at_ms,
                max_archive_bytes,
                validate_inventory,
            )
        })
        .await
        .map_err(|error| blocking_error("snapshot OKF Knowledge storage", error))?
        .map(Some)
    }

    pub(crate) async fn backup_archive_evidence(
        backup_path: impl Into<PathBuf>,
        max_archive_bytes: u64,
    ) -> UseResult<(u64, String)> {
        let backup_path = backup_path.into();
        tokio::task::spawn_blocking(move || {
            backup::archive_file_evidence(&backup_path, max_archive_bytes)
        })
        .await
        .map_err(|error| blocking_error("hash an OKF Knowledge backup archive", error))?
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

    /// Build a bounded, oldest-first retention plan for verified backups in
    /// one owned directory and one complete scope. No backup is removed.
    pub async fn plan_backup_retention(
        directory: impl Into<PathBuf>,
        scope: &PlanScope,
        policy: OkfKnowledgeBackupRetentionPolicy,
    ) -> UseResult<OkfKnowledgeBackupRetentionPlan> {
        let directory = directory.into();
        let scope = scope.clone();
        tokio::task::spawn_blocking(move || backup_retention::plan(&directory, &scope, policy))
            .await
            .map_err(|error| blocking_error("plan OKF Knowledge backup retention", error))?
    }

    /// Apply only the exact canonical retention plan digest against an
    /// unchanged verified directory inventory.
    pub async fn apply_backup_retention(
        directory: impl Into<PathBuf>,
        scope: &PlanScope,
        policy: OkfKnowledgeBackupRetentionPolicy,
        expected_plan_digest: impl Into<String>,
    ) -> UseResult<OkfKnowledgeBackupRetentionResult> {
        let directory = directory.into();
        let scope = scope.clone();
        let expected_plan_digest = expected_plan_digest.into();
        tokio::task::spawn_blocking(move || {
            backup_retention::apply(&directory, &scope, policy, expected_plan_digest.as_str())
        })
        .await
        .map_err(|error| blocking_error("apply OKF Knowledge backup retention", error))?
    }

    pub(crate) async fn inspect_backup_for_restore(
        backup_path: impl Into<PathBuf>,
        expected_scope: &PlanScope,
    ) -> UseResult<VerifiedOkfKnowledgeBackup> {
        let backup_path = backup_path.into();
        let expected_scope = expected_scope.clone();
        tokio::task::spawn_blocking(move || backup::inspect(&backup_path, &expected_scope))
            .await
            .map_err(|error| blocking_error("inspect an OKF Knowledge restore backup", error))?
    }

    pub(crate) async fn inspect_staged_restore_database(
        &self,
        database_path: impl Into<PathBuf>,
        manifest: &OkfKnowledgeBackupManifest,
    ) -> UseResult<OkfKnowledgeRestoreInventory> {
        let database_path = database_path.into();
        let manifest = manifest.clone();
        tokio::task::spawn_blocking(move || {
            backup::inspect_restore_database(&database_path, &manifest)
        })
        .await
        .map_err(|error| blocking_error("inspect a staged OKF Knowledge restore database", error))?
    }

    pub(crate) async fn database_file_evidence(
        &self,
        scope: &PlanScope,
    ) -> UseResult<
        Option<(
            u64,
            String,
            bool,
            Option<(u64, String)>,
            Option<(u64, String)>,
        )>,
    > {
        let guard = self.database_guard(scope, LockMode::Exclusive).await?;
        match tokio::fs::symlink_metadata(&guard.path).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_file() => {}
            Ok(_) => {
                return Err(UseError::new(
                    "use.okf.knowledge_database_path_invalid",
                    "The Knowledge database path is not an owned regular file.",
                ))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(UseError::new(
                    "use.okf.knowledge_database_io",
                    format!(
                        "Failed to inspect Knowledge database '{}': {error}",
                        guard.path.display()
                    ),
                ))
            }
        }
        let scope = scope.clone();
        let policy = self.policy;
        tokio::task::spawn_blocking(move || {
            let evidence = || -> UseResult<_> {
                let (bytes, sha256) = backup::file_evidence(&guard.path)?;
                let wal = backup::optional_file_evidence(&sidecar_path(&guard.path, "-wal"))?;
                let shm = backup::optional_file_evidence(&sidecar_path(&guard.path, "-shm"))?;
                Ok((bytes, sha256, wal, shm))
            };
            let before = evidence()?;
            let temporary = tempfile::tempdir().map_err(|error| {
                UseError::new(
                    "use.okf.knowledge_restore_backend_failed",
                    format!("Failed to create a temporary restore-audit directory: {error}"),
                )
            })?;
            let snapshot = temporary.path().join("knowledge.sqlite3");
            copy_restore_evidence(&guard.path, &snapshot, Some((&before.0, &before.1)))?;
            copy_restore_evidence(
                &sidecar_path(&guard.path, "-wal"),
                &sidecar_path(&snapshot, "-wal"),
                before.2.as_ref().map(|(bytes, sha256)| (bytes, sha256)),
            )?;
            copy_restore_evidence(
                &sidecar_path(&guard.path, "-shm"),
                &sidecar_path(&snapshot, "-shm"),
                before.3.as_ref().map(|(bytes, sha256)| (bytes, sha256)),
            )?;
            let integrity_verified = schema::open(&snapshot, false)
                .map(|connection| audit::audit(&connection, &scope, &policy).is_ok())
                .unwrap_or(false);
            let after = evidence()?;
            if after != before {
                return Err(UseError::new(
                    "use.okf.knowledge_restore_database_changed",
                    "The Knowledge database or its sidecars changed while read-only restore evidence was collected.",
                ));
            }
            let (bytes, sha256, wal, shm) = before;
            Ok((bytes, sha256, integrity_verified, wal, shm))
        })
        .await
        .map_err(|error| blocking_error("hash the current OKF Knowledge database", error))?
        .map(Some)
    }

    pub(crate) fn scope_directory(&self, scope: &PlanScope) -> UseResult<PathBuf> {
        self.installation.ensure_same(scope)?;
        if scope.validate().is_err() {
            return Err(UseError::new(
                "use.okf.knowledge_database_scope_invalid",
                "The Knowledge database requires a valid complete User or Workspace scope.",
            ));
        }
        let digest = scope.storage_key().map_err(|_| {
            UseError::new(
                "use.okf.knowledge_database_scope_invalid",
                "The Knowledge database requires a valid complete User or Workspace installation identity.",
            )
        })?;
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

    pub(crate) async fn restore_database_guard(
        &self,
        scope: &PlanScope,
    ) -> UseResult<ScopeDatabaseGuard> {
        self.database_guard(scope, LockMode::Exclusive).await
    }

    fn maintenance_lock(&self) -> a3s_use_extension::StateMaintenanceLock {
        a3s_use_extension::StateMaintenanceLock::new(&self.state_root)
    }
}

fn sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut value = OsString::from(database.as_os_str());
    value.push(suffix);
    PathBuf::from(value)
}

fn copy_restore_evidence(
    source: &Path,
    destination: &Path,
    expected: Option<(&u64, &String)>,
) -> UseResult<()> {
    let Some((expected_bytes, expected_sha256)) = expected else {
        return Ok(());
    };
    let copied = std::fs::copy(source, destination).map_err(|error| {
        UseError::new(
            "use.okf.knowledge_restore_database_changed",
            format!(
                "Failed to copy stable Knowledge restore evidence '{}': {error}",
                source.display()
            ),
        )
    })?;
    let actual = backup::optional_file_evidence(destination)?.ok_or_else(|| {
        UseError::new(
            "use.okf.knowledge_restore_database_changed",
            "Copied Knowledge restore evidence disappeared before audit.",
        )
    })?;
    if copied != *expected_bytes || actual.0 != *expected_bytes || actual.1 != *expected_sha256 {
        return Err(UseError::new(
            "use.okf.knowledge_restore_database_changed",
            "Knowledge restore evidence changed while it was copied for offline audit.",
        ));
    }
    Ok(())
}

async fn require_database(path: &Path) -> UseResult<()> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(())
        }
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
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

    async fn read(&self, request: &OkfKnowledgeReadRequest) -> UseResult<OkfKnowledgeReadResponse> {
        let _maintenance = self.maintenance_lock().acquire_shared().await?;
        request.validate()?;
        let request = request.clone();
        let guard = self
            .database_guard(&request.scope, LockMode::Shared)
            .await?;
        tokio::task::spawn_blocking(move || {
            let connection = schema::open(&guard.path, false)
                .map_err(|error| sqlite_error("open cited-read Knowledge database", error))?;
            read::read(&connection, &request)
        })
        .await
        .map_err(|error| blocking_error("read the cited OKF Knowledge document", error))?
    }
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
mod backup_retention_tests;
#[cfg(test)]
mod tests;
