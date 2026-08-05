use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{PluginPackageId, UseError, UseResult};
use a3s_use_extension::ExtensionPaths;
use fs2::FileExt;
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::journal::{record_error, PluginLifecycleCheckpointOutcome};
use super::model::{valid_machine_id, PluginLifecycleIntent};
use super::PluginLifecycleOperationRecord;

const MAX_OPERATION_BYTES: u64 = 1024 * 1024;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

/// Durable, cross-process journal for one package-level lifecycle saga.
///
/// Records contain no Secret values, provider credentials, endpoint tokens,
/// or package-authored error text. A retry of the same operation resumes the
/// exact next checkpoint; a different operation for the same scope/package is
/// rejected until the current operation reaches a terminal record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginLifecycleJournalStore {
    state_root: PathBuf,
    root: PathBuf,
}

impl PluginLifecycleJournalStore {
    pub fn new(state_root: impl Into<PathBuf>) -> Self {
        let state_root = state_root.into();
        Self {
            root: state_root.join("operations").join("plugins"),
            state_root,
        }
    }

    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        Self::new(paths.state_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub async fn begin(
        &self,
        intent: &PluginLifecycleIntent,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        intent.validate()?;
        let directory = self.package_directory(&intent.scope_id, &intent.package_id)?;
        let _lock = acquire_lock(&self.state_root, &directory).await?;
        let active_path = directory.join("active.json");
        if let Some(current) = read_optional_record(&active_path).await? {
            validate_record_ownership(&current, &intent.scope_id, &intent.package_id)?;
            if current.intent == *intent {
                return Ok(current);
            }
            if current.status == super::PluginLifecycleOperationStatus::Applying {
                return Err(store_error(
                    "use.plugin.lifecycle_busy",
                    "Another lifecycle operation is still active for this cognitive package.",
                ));
            }
            write_record(&directory.join("last.json"), &current).await?;
        }
        let record = PluginLifecycleOperationRecord::new(intent.clone())?;
        write_record(&active_path, &record).await?;
        Ok(record)
    }

    pub async fn load_active(
        &self,
        scope_id: &str,
        package_id: &str,
    ) -> UseResult<Option<PluginLifecycleOperationRecord>> {
        let directory = self.package_directory(scope_id, package_id)?;
        if !validate_existing_directory_chain(&self.state_root, &directory).await? {
            return Ok(None);
        }
        let record = read_optional_record(&directory.join("active.json")).await?;
        if let Some(record) = &record {
            validate_record_ownership(record, scope_id, package_id)?;
        }
        Ok(record)
    }

    pub async fn load_last(
        &self,
        scope_id: &str,
        package_id: &str,
    ) -> UseResult<Option<PluginLifecycleOperationRecord>> {
        let directory = self.package_directory(scope_id, package_id)?;
        if !validate_existing_directory_chain(&self.state_root, &directory).await? {
            return Ok(None);
        }
        let record = read_optional_record(&directory.join("last.json")).await?;
        if let Some(record) = &record {
            validate_record_ownership(record, scope_id, package_id)?;
        }
        Ok(record)
    }

    pub async fn record_checkpoint(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
        outcome: PluginLifecycleCheckpointOutcome,
        evidence_digest: impl Into<String>,
        error_code: Option<String>,
        completed_at_ms: u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        let evidence_digest = evidence_digest.into();
        self.update(intent, |record| {
            record.record_checkpoint(
                idempotency_key,
                outcome,
                evidence_digest,
                error_code,
                completed_at_ms,
            )?;
            Ok(())
        })
        .await
    }

    pub async fn record_failure(
        &self,
        intent: &PluginLifecycleIntent,
        idempotency_key: &str,
        error_code: impl Into<String>,
        evidence_digest: impl Into<String>,
        failed_at_ms: u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        let error_code = error_code.into();
        let evidence_digest = evidence_digest.into();
        self.update(intent, |record| {
            record.record_failure(idempotency_key, error_code, evidence_digest, failed_at_ms)
        })
        .await
    }

    pub async fn complete(
        &self,
        intent: &PluginLifecycleIntent,
        completed_at_ms: u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        self.update(intent, |record| {
            record.complete(completed_at_ms)?;
            Ok(())
        })
        .await
    }

    pub async fn roll_back(
        &self,
        intent: &PluginLifecycleIntent,
        evidence_digest: impl Into<String>,
        completed_at_ms: u64,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        let evidence_digest = evidence_digest.into();
        self.update(intent, |record| {
            record.roll_back(evidence_digest, completed_at_ms)?;
            Ok(())
        })
        .await
    }

    pub async fn start_rollback(
        &self,
        intent: &PluginLifecycleIntent,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        self.update(intent, |record| {
            record.start_rollback()?;
            Ok(())
        })
        .await
    }

    async fn update(
        &self,
        intent: &PluginLifecycleIntent,
        update: impl FnOnce(&mut PluginLifecycleOperationRecord) -> UseResult<()>,
    ) -> UseResult<PluginLifecycleOperationRecord> {
        intent.validate()?;
        let directory = self.package_directory(&intent.scope_id, &intent.package_id)?;
        let _lock = acquire_lock(&self.state_root, &directory).await?;
        let active_path = directory.join("active.json");
        let mut record = read_optional_record(&active_path).await?.ok_or_else(|| {
            store_error(
                "use.plugin.lifecycle_operation_missing",
                "The cognitive-package lifecycle operation does not exist.",
            )
        })?;
        validate_record_ownership(&record, &intent.scope_id, &intent.package_id)?;
        if record.intent != *intent {
            return Err(store_error(
                "use.plugin.lifecycle_operation_conflict",
                "The durable lifecycle operation does not match the requested intent.",
            ));
        }
        update(&mut record)?;
        record.validate()?;
        write_record(&active_path, &record).await?;
        Ok(record)
    }

    fn package_directory(&self, scope_id: &str, package_id: &str) -> UseResult<PathBuf> {
        if !valid_machine_id(scope_id) {
            return Err(path_identity_error());
        }
        let package_id = PluginPackageId::parse(package_id.to_string())?;
        let (publisher, package) = package_id
            .as_str()
            .split_once('/')
            .ok_or_else(path_identity_error)?;
        let scope_digest = format!("{:x}", Sha256::digest(scope_id.as_bytes()));
        Ok(self.root.join(scope_digest).join(publisher).join(package))
    }
}

async fn acquire_lock(state_root: &Path, directory: &Path) -> UseResult<StdFile> {
    ensure_owned_directory(state_root, directory).await?;
    let lock_path = directory.join(".operation.lock");
    match fs::symlink_metadata(&lock_path).await {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(path_identity_error())
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect lifecycle lock", &lock_path, error)),
    }
    let error_path = lock_path.clone();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        file.lock_exclusive()?;
        Ok::<_, io::Error>(file)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.plugin.lifecycle_io",
            format!(
                "Failed to acquire lifecycle lock '{}': blocking task failed: {error}",
                error_path.display()
            ),
        )
    })?
    .map_err(|error| path_error("acquire lifecycle lock", &error_path, error))
}

async fn read_optional_record(path: &Path) -> UseResult<Option<PluginLifecycleOperationRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect lifecycle record", path, error)),
    };
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_OPERATION_BYTES
    {
        return Err(record_error(format!(
            "Lifecycle record '{}' is not a bounded regular file.",
            path.display()
        )));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read lifecycle record", path, error))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_OPERATION_BYTES {
        return Err(record_error(
            "A lifecycle record changed outside its size bound while reading.",
        ));
    }
    let record =
        serde_json::from_slice::<PluginLifecycleOperationRecord>(&bytes).map_err(|error| {
            record_error(format!(
                "Lifecycle record '{}' is invalid JSON: {error}",
                path.display()
            ))
        })?;
    record.validate().map_err(|error| {
        record_error(format!(
            "Lifecycle record '{}' is invalid: {}",
            path.display(),
            error.message
        ))
    })?;
    Ok(Some(record))
}

async fn write_record(path: &Path, record: &PluginLifecycleOperationRecord) -> UseResult<()> {
    record.validate()?;
    let bytes = serde_json::to_vec_pretty(record)
        .map_err(|error| record_error(format!("Failed to encode lifecycle record: {error}")))?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_OPERATION_BYTES {
        return Err(record_error(
            "The lifecycle operation exceeds its storage bound.",
        ));
    }
    let parent = path.parent().ok_or_else(path_identity_error)?;
    let temporary = parent.join(format!(".operation-{}.tmp", unique_suffix()));
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = options
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary lifecycle record", &temporary, error))?;
    if let Err(error) = file.write_all(&bytes).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "write temporary lifecycle record",
            &temporary,
            error,
        ));
    }
    if let Err(error) = file.sync_all().await {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error(
            "sync temporary lifecycle record",
            &temporary,
            error,
        ));
    }
    drop(file);
    if let Err(error) = activate_temporary(temporary.clone(), path.to_path_buf()).await {
        let _ = fs::remove_file(&temporary).await;
        return Err(error);
    }
    sync_parent(parent).await
}

async fn ensure_owned_directory(root: &Path, directory: &Path) -> UseResult<()> {
    if !directory.starts_with(root) {
        return Err(path_identity_error());
    }
    fs::create_dir_all(root)
        .await
        .map_err(|error| path_error("create lifecycle state root", root, error))?;
    validate_directory(root).await?;
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| path_identity_error())?;
    let mut current = root.to_path_buf();
    for segment in relative.components() {
        current.push(segment.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create lifecycle operation directory",
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
        return Err(path_identity_error());
    }
    let relative = directory
        .strip_prefix(root)
        .map_err(|_| path_identity_error())?;
    let mut current = root.to_path_buf();
    for segment in std::iter::once(None).chain(relative.components().map(Some)) {
        if let Some(segment) = segment {
            current.push(segment.as_os_str());
        }
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => {}
            Ok(_) => return Err(path_identity_error()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => {
                return Err(path_error(
                    "inspect lifecycle operation directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(true)
}

async fn activate_temporary(temporary: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        let temporary = tempfile::TempPath::try_from_path(temporary)?;
        temporary.persist(target).map_err(|error| error.error)
    })
    .await
    .map_err(|error| {
        store_error(
            "use.plugin.lifecycle_io",
            format!(
                "Failed to activate lifecycle record '{}': blocking task failed: {error}",
                error_target.display()
            ),
        )
    })?
    .map_err(|error| path_error("activate lifecycle record", &error_target, error))
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect lifecycle directory", path, error))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(path_identity_error());
    }
    Ok(())
}

fn validate_record_ownership(
    record: &PluginLifecycleOperationRecord,
    scope_id: &str,
    package_id: &str,
) -> UseResult<()> {
    if record.intent.scope_id != scope_id || record.intent.package_id != package_id {
        return Err(store_error(
            "use.plugin.lifecycle_ownership_mismatch",
            "A lifecycle record does not match its scope and package path.",
        ));
    }
    Ok(())
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open lifecycle directory", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync lifecycle directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn path_identity_error() -> UseError {
    store_error(
        "use.plugin.lifecycle_path_invalid",
        "A lifecycle scope, package identity, or owned path is invalid.",
    )
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_error(
        "use.plugin.lifecycle_io",
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

#[cfg(test)]
mod tests;
