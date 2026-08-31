use std::collections::BTreeSet;
use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{PlanScope, PluginPackageId, UseError, UseResult};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::diagnostic::{
    bounded_count, diagnose_enablement_operation, diagnostic_state_error,
    PluginOperationDiagnostic, PluginOperationHistoryDiagnostic, PluginRetainedOperationDiagnostic,
    PluginRetainedOperationOutcome, MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS,
    MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES, PLUGIN_OPERATION_HISTORY_DIAGNOSTIC_SCHEMA,
};
use super::enablement_store::PendingCognitivePackageEnablement;
use super::store::{PendingPackageGraphOperation, PendingPackageGraphStore};
use super::CognitivePackageManager;

const PLUGIN_OPERATION_HISTORY_SCHEMA: &str = "a3s.use.plugin-operation-history.v1";
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable, non-authoritative observations retained oldest-first.
///
/// The public projection reverses this inventory for operator convenience.
/// Storing the already validated public operation diagnostic avoids copying
/// credentials, paths, package content, or recovery authority into a second
/// private telemetry format.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredPluginOperationHistory {
    schema: String,
    scope: PlanScope,
    package_id: String,
    operations: Vec<PluginRetainedOperationDiagnostic>,
}

impl StoredPluginOperationHistory {
    fn new(scope: PlanScope, package_id: String) -> UseResult<Self> {
        validate_scope(&scope)?;
        PluginPackageId::parse(package_id.clone()).map_err(|_| history_invalid())?;
        Ok(Self {
            schema: PLUGIN_OPERATION_HISTORY_SCHEMA.to_owned(),
            scope,
            package_id,
            operations: Vec::new(),
        })
    }

    fn validate(&self) -> UseResult<()> {
        validate_scope(&self.scope)?;
        PluginPackageId::parse(self.package_id.clone()).map_err(|_| history_invalid())?;
        if self.schema != PLUGIN_OPERATION_HISTORY_SCHEMA
            || self.operations.is_empty()
            || self.operations.len() > MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS
        {
            return Err(history_invalid());
        }
        let mut operation_occurrences = BTreeSet::new();
        for retained in &self.operations {
            retained.validate().map_err(|_| history_invalid())?;
            let operation = &retained.diagnostic;
            if operation.scope != self.scope
                || operation.package_id != self.package_id
                || !operation_occurrences.insert((
                    operation.operation.operation_id.as_str(),
                    operation.operation.plan_digest.as_str(),
                ))
            {
                return Err(history_invalid());
            }
        }
        if encoded_history(self)?.len() > MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES {
            return Err(history_invalid());
        }
        Ok(())
    }

    fn retain(
        &mut self,
        diagnostic: &PluginOperationDiagnostic,
        outcome: PluginRetainedOperationOutcome,
    ) -> UseResult<bool> {
        self.retain_with_limits(
            diagnostic,
            outcome,
            MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS,
            MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES,
        )
    }

    fn retain_with_limits(
        &mut self,
        diagnostic: &PluginOperationDiagnostic,
        outcome: PluginRetainedOperationOutcome,
        item_limit: usize,
        byte_limit: usize,
    ) -> UseResult<bool> {
        diagnostic.validate().map_err(|_| history_invalid())?;
        if item_limit == 0
            || byte_limit == 0
            || diagnostic.scope != self.scope
            || diagnostic.package_id != self.package_id
        {
            return Err(history_invalid());
        }
        if let Some(existing) = self.operations.iter().find(|existing| {
            existing.diagnostic.operation.operation_id == diagnostic.operation.operation_id
                && existing.diagnostic.operation.plan_digest == diagnostic.operation.plan_digest
        }) {
            if existing.diagnostic.operation.action != diagnostic.operation.action
                || existing.outcome != outcome
            {
                return Err(history_invalid());
            }
            return Ok(false);
        }

        self.operations.push(PluginRetainedOperationDiagnostic {
            retained_at_ms: diagnostic.observed_at_ms,
            outcome,
            diagnostic: diagnostic.clone(),
        });
        while self.operations.len() > item_limit {
            self.operations.remove(0);
        }
        while encoded_history(self)?.len() > byte_limit && self.operations.len() > 1 {
            self.operations.remove(0);
        }
        self.validate()?;
        Ok(true)
    }
}

#[derive(Debug, Clone)]
pub(super) struct PluginOperationHistoryStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl PluginOperationHistoryStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("operations/package-diagnostic-history"),
            state_root,
        }
    }

    pub async fn retain(
        &self,
        diagnostic: &PluginOperationDiagnostic,
        outcome: PluginRetainedOperationOutcome,
    ) -> UseResult<bool> {
        diagnostic.validate().map_err(|_| history_invalid())?;
        let path = history_path(&self.root, &diagnostic.scope, &diagnostic.package_id)?;
        let lock_path = history_lock_path(&self.root, &diagnostic.scope, &diagnostic.package_id)?;
        let lock_parent = lock_path.parent().ok_or_else(history_invalid)?;
        ensure_owned_directory(&self.state_root, lock_parent).await?;
        let _guard = acquire_lock(lock_path).await?;

        let mut history = read_optional(&self.state_root, &path).await?.unwrap_or(
            StoredPluginOperationHistory::new(
                diagnostic.scope.clone(),
                diagnostic.package_id.clone(),
            )?,
        );
        if !history.operations.is_empty() {
            history.validate()?;
        }
        if !history.retain(diagnostic, outcome)? {
            return Ok(false);
        }
        write_replace(&self.state_root, &path, &history).await?;
        Ok(true)
    }

    pub async fn get(
        &self,
        scope: &PlanScope,
        package_id: &str,
    ) -> UseResult<Vec<PluginRetainedOperationDiagnostic>> {
        validate_scope(scope)?;
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| history_invalid())?;
        let path = history_path(&self.root, scope, package_id)?;
        let Some(history) = read_optional(&self.state_root, &path).await? else {
            return Ok(Vec::new());
        };
        history.validate()?;
        if history.scope != *scope || history.package_id != package_id {
            return Err(history_invalid());
        }
        Ok(history.operations.into_iter().rev().collect())
    }
}

impl CognitivePackageManager {
    fn operation_history_store(&self) -> PluginOperationHistoryStore {
        PluginOperationHistoryStore::new(self.registry.paths().installation_state_root())
    }

    /// Read bounded, newest-first retired operation history without network
    /// access, reconciliation, recovery, or writes.
    pub async fn diagnose_operation_history(
        &self,
        package_id: &str,
    ) -> UseResult<PluginOperationHistoryDiagnostic> {
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            UseError::new(
                "use.plugin.operation_diagnostic_invalid",
                "The operation history package identity is invalid.",
            )
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let operations = self
            .operation_history_store()
            .get(self.scope(), package_id)
            .await
            .map_err(|_| diagnostic_state_error())?;
        let diagnostic = PluginOperationHistoryDiagnostic {
            schema: PLUGIN_OPERATION_HISTORY_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: self.scope().clone(),
            package_id: package_id.to_owned(),
            retention_limit: bounded_count(
                MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS,
                "operation history retention",
            )?,
            retention_byte_limit: u64::try_from(MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES)
                .map_err(|_| diagnostic_state_error())?,
            retained_operation_count: bounded_count(operations.len(), "retained operation")?,
            operations,
        };
        diagnostic
            .validate()
            .map_err(|_| diagnostic_state_error())?;
        Ok(diagnostic)
    }

    /// Persist the final read-only graph projection before deleting recovery
    /// authority. A crash after retention but before deletion is safe: replay
    /// sees the same operation ID and does not append a duplicate.
    pub(super) async fn retain_graph_operation_diagnostic(
        &self,
        pending: &PendingPackageGraphOperation,
        outcome: PluginRetainedOperationOutcome,
    ) -> UseResult<bool> {
        let diagnostic = self
            .diagnose_graph_operation(pending.root_package_id(), pending.clone())
            .await?;
        self.operation_history_store()
            .retain(&diagnostic, outcome)
            .await
    }

    pub(super) async fn retain_and_remove_graph_operation(
        &self,
        store: &PendingPackageGraphStore,
        pending: &PendingPackageGraphOperation,
        outcome: PluginRetainedOperationOutcome,
    ) -> UseResult<bool> {
        self.retain_graph_operation_diagnostic(pending, outcome)
            .await?;
        store.remove(pending).await
    }

    /// Retain a completed enable/disable projection while its exact active
    /// state still exists. Callers persist the terminal operation first and
    /// clear active recovery authority only after this returns.
    pub(super) async fn retain_enablement_operation_diagnostic(
        &self,
        active: &PendingCognitivePackageEnablement,
    ) -> UseResult<bool> {
        let diagnostic =
            diagnose_enablement_operation(self, active.request.package_id.as_str(), active.clone())
                .await?;
        self.operation_history_store()
            .retain(&diagnostic, PluginRetainedOperationOutcome::Completed)
            .await
    }
}

fn history_path(root: &Path, scope: &PlanScope, package_id: &str) -> UseResult<PathBuf> {
    Ok(root
        .join("scopes")
        .join(scope_digest(scope)?)
        .join(package_relative_path(package_id, "json")?))
}

fn history_lock_path(root: &Path, scope: &PlanScope, package_id: &str) -> UseResult<PathBuf> {
    Ok(root
        .join("locks")
        .join(scope_digest(scope)?)
        .join(package_relative_path(package_id, "lock")?))
}

fn package_relative_path(package_id: &str, extension: &str) -> UseResult<PathBuf> {
    PluginPackageId::parse(package_id.to_owned()).map_err(|_| history_invalid())?;
    let (publisher, package) = package_id.split_once('/').ok_or_else(history_invalid)?;
    Ok(Path::new(publisher).join(format!("{package}.{extension}")))
}

fn scope_digest(scope: &PlanScope) -> UseResult<String> {
    scope.storage_key().map_err(|_| history_invalid())
}

fn validate_scope(scope: &PlanScope) -> UseResult<()> {
    scope.validate().map_err(|_| history_invalid())
}

pub(super) fn validate_snapshot_record(
    relative_path: &str,
    bytes: &[u8],
    installation: &a3s_use_core::InstallationId,
) -> UseResult<String> {
    if bytes.is_empty() || bytes.len() > MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES {
        return Err(history_invalid());
    }
    let history: StoredPluginOperationHistory =
        serde_json::from_slice(bytes).map_err(|_| history_invalid())?;
    history.validate()?;
    installation
        .ensure_same(&history.scope)
        .map_err(|_| history_invalid())?;
    let expected = history_path(Path::new(""), &history.scope, history.package_id.as_str())?;
    if expected.to_string_lossy().replace('\\', "/") != relative_path {
        return Err(history_invalid());
    }
    Ok(history.package_id)
}

#[cfg(test)]
pub(super) fn snapshot_fixture(installation: &a3s_use_core::InstallationId) -> (String, Vec<u8>) {
    let mut diagnostic =
        crate::cognitive_package::diagnostic::tests::completed_operation_diagnostic();
    diagnostic.scope = installation.clone();
    diagnostic.validate().unwrap();
    let mut history =
        StoredPluginOperationHistory::new(installation.clone(), diagnostic.package_id.clone())
            .unwrap();
    history
        .retain(&diagnostic, PluginRetainedOperationOutcome::Completed)
        .unwrap();
    let relative = history_path(Path::new(""), installation, &diagnostic.package_id)
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let mut bytes = encoded_history(&history).unwrap();
    bytes.push(b'\n');
    (format!("package-diagnostic-history/{relative}"), bytes)
}

async fn acquire_lock(path: PathBuf) -> UseResult<StdFile> {
    match fs::symlink_metadata(&path).await {
        Ok(metadata)
            if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                || !metadata.is_file() =>
        {
            return Err(history_invalid())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect operation history lock", &path, error)),
    }
    let error_path = path.clone();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)?;
        file.lock_exclusive()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(|error| {
        history_io(format!(
            "Failed to join operation history lock task '{}': {error}",
            error_path.display()
        ))
    })?
    .map_err(|error| path_error("acquire operation history lock", &error_path, error))
}

async fn read_optional(
    state_root: &Path,
    path: &Path,
) -> UseResult<Option<StoredPluginOperationHistory>> {
    if !path.starts_with(state_root) || path == state_root {
        return Err(history_invalid());
    }
    let parent = path.parent().ok_or_else(history_invalid)?;
    if !validate_existing_directory_chain(state_root, parent).await? {
        return Ok(None);
    }
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect operation history", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES as u64
    {
        return Err(history_invalid());
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read operation history", path, error))?;
    if bytes.is_empty() || bytes.len() > MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES {
        return Err(history_invalid());
    }
    let history: StoredPluginOperationHistory =
        serde_json::from_slice(&bytes).map_err(|_| history_invalid())?;
    history.validate()?;
    Ok(Some(history))
}

async fn write_replace(
    state_root: &Path,
    path: &Path,
    history: &StoredPluginOperationHistory,
) -> UseResult<()> {
    history.validate()?;
    if !path.starts_with(state_root) || path == state_root {
        return Err(history_invalid());
    }
    let bytes = encoded_history(history)?;
    let parent = path.parent().ok_or_else(history_invalid)?;
    ensure_owned_directory(state_root, parent).await?;
    let temporary = parent.join(format!(".operation-history-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary operation history", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(&bytes).await?;
        file.write_all(b"\n").await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "commit temporary operation history",
            path,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_replace(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(parent).await
}

fn encoded_history(history: &StoredPluginOperationHistory) -> UseResult<Vec<u8>> {
    let bytes = serde_json::to_vec_pretty(history).map_err(|_| history_invalid())?;
    if bytes.is_empty() {
        return Err(history_invalid());
    }
    Ok(bytes)
}

async fn activate_replace(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_replace_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        history_io(format!(
            "Failed to join operation history activation for '{}': {error}",
            error_target.display()
        ))
    })?
    .map_err(|error| path_error("activate operation history", &error_target, error))
}

async fn ensure_owned_directory(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(history_invalid());
    }
    fs::create_dir_all(root)
        .await
        .map_err(|error| path_error("create operation history root", root, error))?;
    validate_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| history_invalid())?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create operation history directory",
                    &current,
                    error,
                ))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory_chain(root: &Path, directory: &Path) -> UseResult<bool> {
    if !directory.starts_with(root) {
        return Err(history_invalid());
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| history_invalid())?;
    let mut current = root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata)
                if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    && metadata.is_dir() => {}
            Ok(_) => return Err(history_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect operation history directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect operation history directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(history_invalid());
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open operation history directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync operation history directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn unique_suffix() -> String {
    let time = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{time}-{sequence}", std::process::id())
}

fn history_invalid() -> UseError {
    UseError::new(
        "use.plugin.operation_history_store_invalid",
        "The retained cognitive-package operation history is unsupported, damaged, or internally inconsistent.",
    )
}

fn history_io(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.operation_history_store_io", message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    history_io(format!("Failed to {action} '{}': {error}", path.display()))
}

#[cfg(test)]
mod byte_retention_tests {
    use super::*;
    use crate::cognitive_package::diagnostic::tests::completed_operation_diagnostic;

    #[test]
    fn byte_limit_prunes_the_oldest_entry_before_validation() {
        let first = completed_operation_diagnostic();
        let mut history =
            StoredPluginOperationHistory::new(first.scope.clone(), first.package_id.clone())
                .unwrap();
        history
            .retain_with_limits(
                &first,
                PluginRetainedOperationOutcome::Completed,
                MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS,
                MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES,
            )
            .unwrap();
        let one_entry_bytes = encoded_history(&history).unwrap().len();

        let mut second = first.clone();
        second.observed_at_ms += 1;
        second.operation.operation_id = "install:acme-root:0002".to_owned();
        second.operation.plan_digest = format!("sha256:{}", "8".repeat(64));
        second.validate().unwrap();
        history
            .retain_with_limits(
                &second,
                PluginRetainedOperationOutcome::Completed,
                MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS,
                one_entry_bytes,
            )
            .unwrap();

        assert_eq!(history.operations.len(), 1);
        assert_eq!(history.operations[0].diagnostic, second);
        assert!(encoded_history(&history).unwrap().len() <= one_entry_bytes);
    }
}
