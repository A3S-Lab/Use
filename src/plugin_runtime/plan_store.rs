//! Host-owned durable storage for committed Runtime surface plans.
//!
//! A Runtime plan is immutable planning evidence. It is not a package
//! receipt, a live provider selection, or an activation record, so it has a
//! separate store from the Runtime binding/provisioning files. The store is
//! installation-scoped and addresses records by the canonical digest of the
//! complete RuntimeSurfacePlanKey.

use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};
use async_trait::async_trait;
use fs2::FileExt;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use tokio::fs;
use tokio::io::AsyncWriteExt;

use super::{
    RuntimeSurfacePlan, RuntimeSurfacePlanKey, RuntimeSurfacePlanSource,
    MAX_RUNTIME_SURFACE_PLAN_BYTES,
};

/// Versioned envelope used only by the host-owned plan store.
pub const RUNTIME_SURFACE_PLAN_STORE_SCHEMA: &str = "a3s.use.runtime-surface-plan-store-record.v1";
/// A record contains a key and a bounded canonical Runtime plan. Keep a
/// little room for the envelope and key while retaining a strict upper bound.
pub const MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES: usize = MAX_RUNTIME_SURFACE_PLAN_BYTES + 64 * 1024;
/// Prevent an interrupted or malicious writer from turning one installation's
/// plan directory into an unbounded source of work.
pub const MAX_RUNTIME_SURFACE_PLAN_RECORDS: usize = 4096;
const MAX_PLAN_STORE_DIRECTORY_ENTRIES: usize = MAX_RUNTIME_SURFACE_PLAN_RECORDS * 2;
const PLAN_STORE_DIRECTORY: &str = "runtime-plans";
const PLAN_STORE_LOCK: &str = ".runtime-plans.lock";
const PLAN_STORE_ERROR: &str = "use.plugin.runtime.plan_store_invalid";
const PLAN_STORE_IO: &str = "use.plugin.runtime.plan_store_io";
const PLAN_STORE_CONFLICT: &str = "use.plugin.runtime.plan_store_conflict";
const PLAN_NOT_FOUND: &str = "use.plugin.runtime.plan_not_found";
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeSurfacePlanStoreRecord {
    schema: String,
    key: RuntimeSurfacePlanKey,
    plan: RuntimeSurfacePlan,
}

/// Installation-scoped, host-owned source of immutable Runtime plan payloads.
///
/// The store never accepts a package root and never derives a plan from a
/// receipt. A caller must publish the exact key and plan before a production
/// Control commit can reference them. Publication is idempotent for equal
/// canonical content and rejects replacement of an existing key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSurfacePlanStore {
    installation: InstallationId,
    state_root: PathBuf,
    root: PathBuf,
}

impl RuntimeSurfacePlanStore {
    /// Construct a store over an installation-owned state root.
    pub fn new(state_root: impl Into<PathBuf>, installation: InstallationId) -> UseResult<Self> {
        installation.validate()?;
        let state_root = state_root.into();
        Ok(Self {
            root: state_root.join(PLAN_STORE_DIRECTORY),
            state_root,
            installation,
        })
    }

    /// Construct the store below the exact installation state root selected by
    /// ExtensionPaths.
    pub fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        let state_root = paths.installation_state_root();
        Self {
            installation: paths.installation().clone(),
            root: state_root.join(PLAN_STORE_DIRECTORY),
            state_root,
        }
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    /// Return the host-owned plan directory. The path is an implementation
    /// location only; it is never included in a Runtime plan or capability
    /// contract.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Publish one exact immutable plan record.
    ///
    /// true means a new record was published; false means the exact record
    /// already existed. A different record at the same key is a conflict and
    /// is never replaced.
    pub async fn put(
        &self,
        key: &RuntimeSurfacePlanKey,
        plan: &RuntimeSurfacePlan,
    ) -> UseResult<bool> {
        self.validate_key_scope(key)?;
        validate_record(key, plan)?;
        let bytes = encode_record(key, plan)?;
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        let _lock = self.acquire_lock().await?;
        ensure_owned_directory(&self.state_root, &self.root).await?;
        let path = self.path_for(key)?;
        if let Some(existing) = read_record_at(&path).await? {
            return compare_existing(&existing, &bytes);
        }
        let count = count_records(&self.root).await?;
        if count >= MAX_RUNTIME_SURFACE_PLAN_RECORDS {
            return Err(store_error(
                PLAN_STORE_ERROR,
                format!(
                    "The Runtime plan store reached its retained-record limit of {MAX_RUNTIME_SURFACE_PLAN_RECORDS}."
                ),
            ));
        }
        write_new_record(&self.root, &path, &bytes).await?;
        Ok(true)
    }

    /// Read one exact plan, returning None only when its addressed record is
    /// absent. Existing malformed or substituted records fail closed.
    pub async fn get(&self, key: &RuntimeSurfacePlanKey) -> UseResult<Option<RuntimeSurfacePlan>> {
        self.validate_key_scope(key)?;
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        if !validate_existing_directory(&self.root).await? {
            return Ok(None);
        }
        let path = self.path_for(key)?;
        let Some(record) = read_record_at(&path).await? else {
            return Ok(None);
        };
        if record.key != *key {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan record key differs from its addressed key.",
            ));
        }
        validate_record(&record.key, &record.plan)?;
        Ok(Some(record.plan))
    }

    /// Verify the complete bounded directory inventory without returning host
    /// paths. This is intended for a future backup/restore payload-owner
    /// adapter; it is deliberately not a source of Control authority.
    pub async fn inspect_keys(&self) -> UseResult<Vec<RuntimeSurfacePlanKey>> {
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        if !validate_existing_directory(&self.root).await? {
            return Ok(Vec::new());
        }
        let mut entries = fs::read_dir(&self.root)
            .await
            .map_err(|error| path_error("read Runtime plan store", &self.root, error))?;
        let mut keys = Vec::new();
        let mut entries_seen = 0_usize;
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|error| path_error("read Runtime plan store entry", &self.root, error))?
        {
            entries_seen = entries_seen.saturating_add(1);
            if entries_seen > MAX_PLAN_STORE_DIRECTORY_ENTRIES {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "The Runtime plan store directory exceeds its entry bound.",
                ));
            }
            let file_name = entry.file_name();
            let name = file_name.to_str().ok_or_else(|| {
                store_error(PLAN_STORE_ERROR, "A Runtime plan filename is not UTF-8.")
            })?;
            if name == PLAN_STORE_LOCK {
                validate_regular_file(&entry.path()).await?;
                continue;
            }
            if is_temporary_name(name) {
                validate_temporary_file(&entry.path()).await?;
                continue;
            }
            if !is_record_name(name) {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "The Runtime plan store contains an unknown entry.",
                ));
            }
            let record = read_record_at(&entry.path()).await?.ok_or_else(|| {
                store_error(
                    PLAN_STORE_ERROR,
                    "A Runtime plan record disappeared during inventory.",
                )
            })?;
            self.validate_key_scope(&record.key)?;
            validate_record(&record.key, &record.plan)?;
            let expected = self.path_for(&record.key)?;
            if expected != entry.path() {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "A Runtime plan record is not stored at its canonical key path.",
                ));
            }
            keys.push(record.key);
        }
        keys.sort();
        if keys.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan store contains duplicate key records.",
            ));
        }
        Ok(keys)
    }

    fn validate_key_scope(&self, key: &RuntimeSurfacePlanKey) -> UseResult<()> {
        key.validate()?;
        self.installation.ensure_same(&key.scope)
    }

    fn path_for(&self, key: &RuntimeSurfacePlanKey) -> UseResult<PathBuf> {
        let digest = key.descriptor_digest()?;
        let hex = digest.strip_prefix("sha256:").ok_or_else(|| {
            store_error(PLAN_STORE_ERROR, "A Runtime plan key digest is invalid.")
        })?;
        Ok(self.root.join(format!("{hex}.json")))
    }

    async fn acquire_lock(&self) -> UseResult<StdFile> {
        fs::create_dir_all(&self.state_root)
            .await
            .map_err(|error| {
                path_error("create Runtime plan state root", &self.state_root, error)
            })?;
        validate_directory(&self.state_root).await?;
        ensure_owned_directory(&self.state_root, &self.root).await?;
        let path = self.root.join(PLAN_STORE_LOCK);
        match fs::symlink_metadata(&path).await {
            Ok(metadata)
                if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                    || !metadata.is_file() =>
            {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "The Runtime plan store lock is not an owned regular file.",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(path_error("inspect Runtime plan store lock", &path, error)),
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
            store_error(
                PLAN_STORE_IO,
                format!("Failed to acquire the Runtime plan store lock: {error}"),
            )
        })?
        .map_err(|error| path_error("acquire Runtime plan store lock", &error_path, error))
    }
}

#[async_trait]
impl RuntimeSurfacePlanSource for RuntimeSurfacePlanStore {
    async fn read_plan(&self, key: &RuntimeSurfacePlanKey) -> UseResult<Vec<u8>> {
        let Some(plan) = self.get(key).await? else {
            return Err(store_error(
                PLAN_NOT_FOUND,
                "The committed Runtime surface plan is not present in the host-owned store.",
            ));
        };
        plan.to_canonical_bytes()
    }
}

fn validate_record(key: &RuntimeSurfacePlanKey, plan: &RuntimeSurfacePlan) -> UseResult<()> {
    if !key.matches_plan(plan) {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan does not match its complete durable key.",
        ));
    }
    plan.validate().map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!("The Runtime plan store record contains an invalid plan: {error}"),
        )
    })
}

fn encode_record(key: &RuntimeSurfacePlanKey, plan: &RuntimeSurfacePlan) -> UseResult<Vec<u8>> {
    let record = RuntimeSurfacePlanStoreRecord {
        schema: RUNTIME_SURFACE_PLAN_STORE_SCHEMA.to_owned(),
        key: key.clone(),
        plan: plan.clone(),
    };
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    record.serialize(&mut serializer).map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!("Failed to encode the Runtime plan store record: {error}"),
        )
    })?;
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store record exceeds its size bound.",
        ));
    }
    Ok(bytes)
}

async fn read_record_at(path: &Path) -> UseResult<Option<RuntimeSurfacePlanStoreRecord>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect Runtime plan record", path, error)),
    };
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() as usize > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES
    {
        return Err(store_error(
            PLAN_STORE_ERROR,
            format!(
                "Runtime plan record '{}' is not a bounded regular file.",
                path.display()
            ),
        ));
    }
    let bytes = fs::read(path)
        .await
        .map_err(|error| path_error("read Runtime plan record", path, error))?;
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record changed outside its size bound while reading.",
        ));
    }
    let record: RuntimeSurfacePlanStoreRecord =
        serde_json::from_slice(&bytes).map_err(|error| {
            store_error(
                PLAN_STORE_ERROR,
                format!(
                    "Runtime plan record '{}' is invalid JSON: {error}",
                    path.display()
                ),
            )
        })?;
    if record.schema != RUNTIME_SURFACE_PLAN_STORE_SCHEMA {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store record schema is unsupported.",
        ));
    }
    validate_record(&record.key, &record.plan)?;
    let canonical = encode_record(&record.key, &record.plan)?;
    if canonical != bytes {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store record is not canonical JSON.",
        ));
    }
    Ok(Some(record))
}

fn compare_existing(existing: &RuntimeSurfacePlanStoreRecord, requested: &[u8]) -> UseResult<bool> {
    let existing = encode_record(&existing.key, &existing.plan)?;
    if existing == requested {
        Ok(false)
    } else {
        Err(store_error(
            PLAN_STORE_CONFLICT,
            "A Runtime plan key already contains different immutable content.",
        ))
    }
}

async fn write_new_record(root: &Path, path: &Path, bytes: &[u8]) -> UseResult<()> {
    let parent = path.parent().ok_or_else(|| {
        store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record has no owned parent directory.",
        )
    })?;
    ensure_owned_directory(root, parent).await?;
    let temporary = parent.join(format!(".plan-{}.tmp", unique_suffix()));
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .await
        .map_err(|error| path_error("create temporary Runtime plan record", &temporary, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.sync_all().await?;
        Ok::<_, io::Error>(())
    }
    .await
    {
        let _ = fs::remove_file(&temporary).await;
        return Err(path_error("write Runtime plan record", &temporary, error));
    }
    drop(file);
    let target = path.to_path_buf();
    let error_target = target.clone();
    let publish = tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        store_error(
            PLAN_STORE_IO,
            format!(
                "Failed to publish Runtime plan record '{}': {error}",
                error_target.display()
            ),
        )
    })?;
    if let Err(error) = publish {
        if error.kind() == io::ErrorKind::AlreadyExists {
            return Err(store_error(
                PLAN_STORE_CONFLICT,
                "A Runtime plan key appeared during no-clobber publication.",
            ));
        }
        return Err(path_error(
            "publish Runtime plan record",
            &error_target,
            error,
        ));
    }
    sync_parent(parent).await
}

async fn count_records(root: &Path) -> UseResult<usize> {
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read Runtime plan store", root, error))?;
    let mut count = 0_usize;
    let mut seen = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read Runtime plan store entry", root, error))?
    {
        seen = seen.saturating_add(1);
        if seen > MAX_PLAN_STORE_DIRECTORY_ENTRIES {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan store directory exceeds its entry bound.",
            ));
        }
        let file_name = entry.file_name();
        let name = file_name.to_str().ok_or_else(|| {
            store_error(PLAN_STORE_ERROR, "A Runtime plan filename is not UTF-8.")
        })?;
        if name == PLAN_STORE_LOCK {
            validate_regular_file(&entry.path()).await?;
        } else if is_temporary_name(name) {
            validate_temporary_file(&entry.path()).await?;
        } else if is_record_name(name) {
            let record = read_record_at(&entry.path()).await?.ok_or_else(|| {
                store_error(
                    PLAN_STORE_ERROR,
                    "A Runtime plan record disappeared during inventory.",
                )
            })?;
            validate_record(&record.key, &record.plan)?;
            count = count.saturating_add(1);
        } else {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan store contains an unknown entry.",
            ));
        }
    }
    Ok(count)
}

fn is_record_name(name: &str) -> bool {
    let Some(hex) = name.strip_suffix(".json") else {
        return false;
    };
    hex.len() == 64 && hex.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn is_temporary_name(name: &str) -> bool {
    name.starts_with(".plan-") && name.ends_with(".tmp") && name.len() <= 256
}

async fn validate_temporary_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect temporary Runtime plan record", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES as u64
    {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A temporary Runtime plan record is not an owned bounded file.",
        ));
    }
    Ok(())
}

async fn ensure_owned_directory(root: &Path, target: &Path) -> UseResult<()> {
    if !target.starts_with(root) {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan path escapes its host-owned root.",
        ));
    }
    match fs::symlink_metadata(root).await {
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            fs::create_dir_all(root)
                .await
                .map_err(|error| path_error("create Runtime plan store", root, error))?;
        }
        Err(error) => return Err(path_error("inspect Runtime plan store", root, error)),
    }
    validate_directory(root).await?;
    let relative = target
        .strip_prefix(root)
        .map_err(|_| store_error(PLAN_STORE_ERROR, "A Runtime plan path has no owned prefix."))?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        if !matches!(component, std::path::Component::Normal(_)) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan path contains a non-portable component.",
            ));
        }
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(path_error("create Runtime plan directory", &current, error)),
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_existing_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error("inspect Runtime plan store", path, error)),
    }
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect Runtime plan directory", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(store_error(
            PLAN_STORE_ERROR,
            format!(
                "Runtime plan directory '{}' is not an owned directory.",
                path.display()
            ),
        ));
    }
    Ok(())
}

async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect Runtime plan store file", path, error))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_file() {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan store entry is not an owned regular file.",
        ));
    }
    Ok(())
}

#[cfg(unix)]
async fn sync_parent(parent: &Path) -> UseResult<()> {
    fs::File::open(parent)
        .await
        .map_err(|error| path_error("open Runtime plan directory for sync", parent, error))?
        .sync_all()
        .await
        .map_err(|error| path_error("sync Runtime plan directory", parent, error))
}

#[cfg(not(unix))]
async fn sync_parent(_parent: &Path) -> UseResult<()> {
    Ok(())
}

fn unique_suffix() -> String {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{}-{timestamp}-{sequence}", std::process::id())
}

fn store_error(code: &'static str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}

fn path_error(action: &str, path: &Path, error: io::Error) -> UseError {
    store_error(
        PLAN_STORE_IO,
        format!("Failed to {action} '{}': {error}", path.display()),
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeSurfacePlanStore>();
};
