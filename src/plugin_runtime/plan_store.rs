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
use a3s_use_extension::{
    ArtifactStore, ExtensionPaths, StateMaintenanceGuard, StateMaintenanceLock,
};
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
/// Bound one publication request before any host-owned bytes are written.
pub const MAX_RUNTIME_SURFACE_PLAN_BATCH_BYTES: usize = 16 * 1024 * 1024;
const MAX_PLAN_STORE_DIRECTORY_ENTRIES: usize = MAX_RUNTIME_SURFACE_PLAN_RECORDS * 2;
const PLAN_STORE_DIRECTORY: &str = "runtime-plans";
const PLAN_STORE_LOCK: &str = ".runtime-plans.lock";
const PLAN_STORE_ERROR: &str = "use.plugin.runtime.plan_store_invalid";
const PLAN_STORE_IO: &str = "use.plugin.runtime.plan_store_io";
const PLAN_STORE_CONFLICT: &str = "use.plugin.runtime.plan_store_conflict";
const PLAN_NOT_FOUND: &str = "use.plugin.runtime.plan_not_found";
const PLAN_STORE_LOCK_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const PLAN_STORE_LOCK_RETRY_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RuntimeSurfacePlanStoreRecord {
    schema: String,
    key: RuntimeSurfacePlanKey,
    plan: RuntimeSurfacePlan,
}

/// Canonical bytes captured from one installation-scoped plan record.
///
/// The payload-owner snapshot layer deliberately carries bytes rather than a
/// filesystem path.  This keeps the Runtime plan store responsible for
/// decoding and validating its own envelope while the Control snapshot layer
/// can stream those bytes into an archive or a clean restore candidate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RuntimeSurfacePlanStoredRecord {
    pub(crate) key: RuntimeSurfacePlanKey,
    pub(crate) bytes: Vec<u8>,
}

/// One immutable key/payload pair admitted to the host-owned plan store.
///
/// The pair is validated before a publication request acquires a lock. It is
/// intentionally separate from the on-disk envelope so callers cannot supply
/// or depend on a filesystem path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSurfacePlanPublication {
    pub key: RuntimeSurfacePlanKey,
    pub plan: RuntimeSurfacePlan,
}

impl RuntimeSurfacePlanPublication {
    pub fn new(key: RuntimeSurfacePlanKey, plan: RuntimeSurfacePlan) -> UseResult<Self> {
        validate_record(&key, &plan)?;
        Ok(Self { key, plan })
    }
}

/// Result of an idempotent batch publication.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeSurfacePlanPublishResult {
    pub published: usize,
    pub existing: usize,
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
    /// The global Artifact Store is present for stores composed from
    /// `ExtensionPaths`.  A store built with [`Self::new`] is intentionally
    /// useful for isolated state (for example, an offline restore candidate)
    /// and therefore has no global reference boundary to acquire.
    artifact_store: Option<ArtifactStore>,
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
            artifact_store: None,
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
            artifact_store: Some(paths.artifact_store()),
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

    /// Return the installation state root used for the store's maintenance
    /// fence. This is crate-private because callers should normally obtain
    /// the store from `ExtensionPaths`; the Control composition uses it to
    /// prove that payload publication and the database commit share one root.
    #[allow(dead_code)]
    pub(crate) fn state_root(&self) -> &Path {
        &self.state_root
    }

    /// Capture every valid record while an installation-wide exclusive
    /// maintenance fence is held.  The returned values contain no host path
    /// and are sorted by their canonical content-addressed filename.
    pub(crate) async fn snapshot_records_under_maintenance(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<Vec<RuntimeSurfacePlanStoredRecord>> {
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "Runtime plan snapshot requires the exact installation's exclusive maintenance guard.",
            ));
        }
        if !validate_existing_directory(&self.root).await? {
            return Ok(Vec::new());
        }
        self.scan_records()
            .await?
            .into_iter()
            .map(|(_, record)| {
                let bytes = encode_record(&record.key, &record.plan)?;
                Ok(RuntimeSurfacePlanStoredRecord {
                    key: record.key,
                    bytes,
                })
            })
            .collect()
    }

    /// Inspect a caller-owned candidate directory using the same envelope and
    /// path checks as the live store.  Restore owners use this after staging or
    /// publication because a candidate is intentionally outside the live
    /// store's maintenance root.
    pub(crate) async fn inspect_records_at(
        root: &Path,
        installation: &InstallationId,
    ) -> UseResult<Vec<RuntimeSurfacePlanStoredRecord>> {
        installation.validate()?;
        let store = Self {
            installation: installation.clone(),
            state_root: root.to_path_buf(),
            root: root.to_path_buf(),
            artifact_store: None,
        };
        if !validate_existing_directory(root).await? {
            return Ok(Vec::new());
        }
        store
            .scan_records()
            .await?
            .into_iter()
            .map(|(_, record)| {
                Ok(RuntimeSurfacePlanStoredRecord {
                    bytes: encode_record(&record.key, &record.plan)?,
                    key: record.key,
                })
            })
            .collect()
    }

    /// Inspect a live directory while the enclosing installation already owns
    /// its shared maintenance guard. The plan-store lock is acquired as the
    /// second half of the read boundary, so a concurrent publisher cannot
    /// change a record while it is being decoded into reachability evidence.
    pub(crate) async fn inspect_records_unscoped_under_maintenance(
        root: &Path,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<Vec<RuntimeSurfacePlanStoredRecord>> {
        let state_root = root.parent().ok_or_else(|| {
            store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan store root has no enclosing installation state root.",
            )
        })?;
        if root != state_root.join(PLAN_STORE_DIRECTORY) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "Runtime plan inventory must target the canonical installation plan root.",
            ));
        }
        if !maintenance.is_shared_for(state_root) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "Runtime plan inventory requires the enclosing installation's shared maintenance guard.",
            ));
        }
        if !validate_existing_directory(root).await? {
            return Ok(Vec::new());
        }
        // Reachability inspection is read-only.  In particular, do not create
        // a missing plan-store directory or lock as a side effect of a scan.
        // A compliant publisher holds global reference admission before its
        // state fence, so a root without a lock can still be inspected under
        // the collector's enclosing boundary (for example immediately after
        // a clean restore, whose operational lock is intentionally omitted).
        let _lock = acquire_existing_shared_lock_for(state_root, root).await?;
        scan_records_at(root, None)
            .await?
            .into_iter()
            .map(|(_, record)| {
                Ok(RuntimeSurfacePlanStoredRecord {
                    bytes: encode_record(&record.key, &record.plan)?,
                    key: record.key,
                })
            })
            .collect()
    }

    /// Inspect a candidate that is expected to contain only immutable plan
    /// records.  Unlike a live store, a restore candidate must not carry an
    /// operational lock or interrupted temporary file across the activation
    /// boundary.
    pub(crate) async fn inspect_exact_records_at(
        root: &Path,
        installation: &InstallationId,
    ) -> UseResult<Vec<RuntimeSurfacePlanStoredRecord>> {
        validate_exact_record_directory(root).await?;
        Self::inspect_records_at(root, installation).await
    }

    /// Decode and semantically validate one archived record without exposing
    /// the private on-disk envelope to the payload-owner layer.
    pub(crate) fn decode_record_bytes(
        bytes: &[u8],
    ) -> UseResult<(RuntimeSurfacePlanKey, RuntimeSurfacePlan)> {
        let record = decode_record(bytes)?;
        Ok((record.key, record.plan))
    }

    /// Materialize an exact set of records beneath a caller-owned clean
    /// candidate directory.  The directory is intentionally not protected by
    /// a second maintenance lock: the complete restore coordinator already
    /// owns the target's exclusive fence and the candidate is outside all live
    /// state paths.  Replays are no-clobber and reject any extra or substituted
    /// record.
    pub(crate) async fn materialize_records(
        candidate_root: &Path,
        installation: &InstallationId,
        records: &[RuntimeSurfacePlanStoredRecord],
    ) -> UseResult<()> {
        installation.validate()?;
        ensure_owned_directory(candidate_root, candidate_root).await?;
        let candidate = Self {
            installation: installation.clone(),
            state_root: candidate_root.to_path_buf(),
            root: candidate_root.to_path_buf(),
            artifact_store: None,
        };
        let existing = candidate.scan_records().await?;
        let mut expected: Vec<(PathBuf, Vec<u8>, RuntimeSurfacePlanKey)> =
            Vec::with_capacity(records.len());
        for record in records {
            let (key, plan) = Self::decode_record_bytes(&record.bytes)?;
            if key != record.key {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "An archived Runtime plan record key differs from its decoded envelope.",
                ));
            }
            candidate.installation.ensure_same(&key.scope)?;
            let path = candidate.path_for(&key)?;
            let bytes = encode_record(&key, &plan)?;
            if bytes != record.bytes {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "An archived Runtime plan record is not canonical.",
                ));
            }
            expected.push((path, bytes, key));
        }
        expected.sort_by(|left, right| left.0.cmp(&right.0));
        if expected
            .windows(2)
            .any(|pair| pair[0].0 == pair[1].0 || pair[0].2 == pair[1].2)
        {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "An archived Runtime plan set contains duplicate key identities.",
            ));
        }
        if existing.len() > expected.len() {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan restore candidate contains an extra record.",
            ));
        }
        for (path, bytes, _) in expected {
            if let Some((_, current)) = existing.iter().find(|(current, _)| *current == path) {
                compare_existing(current, &bytes)?;
            } else {
                write_new_record(candidate_root, &path, &bytes).await?;
            }
        }
        let final_records = candidate.scan_records().await?;
        if final_records.len() != records.len()
            || final_records
                .iter()
                .map(|(_, record)| &record.key)
                .ne(records.iter().map(|record| &record.key))
        {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan restore candidate differs from its exact archived inventory.",
            ));
        }
        Ok(())
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
        let publication = RuntimeSurfacePlanPublication::new(key.clone(), plan.clone())?;
        let result = self.publish(std::slice::from_ref(&publication)).await?;
        debug_assert_eq!(result.published + result.existing, 1);
        Ok(result.published == 1)
    }

    /// Publish a bounded set of exact immutable plan records.
    ///
    /// Publication is idempotent for equal canonical content and never
    /// replaces an existing key. If a later write fails, already-published
    /// records remain valid and a retry converges on the same result; callers
    /// must therefore treat the operation as a monotonic payload publication,
    /// not as a transaction that can roll bytes back.
    pub async fn publish(
        &self,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<RuntimeSurfacePlanPublishResult> {
        if publications.is_empty() {
            return Ok(RuntimeSurfacePlanPublishResult {
                published: 0,
                existing: 0,
            });
        }
        // A durable Runtime plan retains an Artifact digest.  Stores created
        // from ExtensionPaths therefore enter the global reference-admission
        // boundary before taking their installation fence, so collection
        // cannot derive an inventory between plan publication and visibility.
        // The `new` constructor deliberately remains available for isolated
        // state and has no global Artifact Store to coordinate.
        let _artifact_admission = match &self.artifact_store {
            Some(store) => Some(store.acquire_reference_admission().await?),
            None => None,
        };
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        self.publish_under_maintenance(&_maintenance, publications)
            .await
    }

    /// Publish records while the caller owns the installation-wide shared
    /// maintenance guard. This is intentionally crate-private: the only
    /// caller is the Control transition coordinator, which must keep plan
    /// publication and the following local authority commit under one fence.
    pub(crate) async fn publish_under_maintenance(
        &self,
        maintenance: &StateMaintenanceGuard,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<RuntimeSurfacePlanPublishResult> {
        if !maintenance.is_shared_for(&self.state_root) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "Runtime plan publication requires the shared guard for its installation state root.",
            ));
        }
        let prepared = self.prepare_publications(publications)?;
        if prepared.is_empty() {
            return Ok(RuntimeSurfacePlanPublishResult {
                published: 0,
                existing: 0,
            });
        }
        let _lock = self.acquire_lock().await?;
        ensure_owned_directory(&self.state_root, &self.root).await?;
        let existing_records = self.scan_records().await?;
        let mut published = 0_usize;
        let mut existing = 0_usize;
        let mut pending = Vec::new();
        for (path, bytes, _) in prepared {
            if let Some((_, current)) = existing_records
                .iter()
                .find(|(current_path, _)| *current_path == path)
            {
                compare_existing(current, &bytes)?;
                existing = existing.saturating_add(1);
            } else {
                pending.push((path, bytes));
            }
        }
        if existing_records.len().saturating_add(pending.len()) > MAX_RUNTIME_SURFACE_PLAN_RECORDS {
            return Err(store_error(
                PLAN_STORE_ERROR,
                format!(
                    "The Runtime plan store reached its retained-record limit of {MAX_RUNTIME_SURFACE_PLAN_RECORDS}."
                ),
            ));
        }
        for (path, bytes) in pending {
            write_new_record(&self.root, &path, &bytes).await?;
            published = published.saturating_add(1);
        }
        Ok(RuntimeSurfacePlanPublishResult {
            published,
            existing,
        })
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
        let mut keys = self
            .scan_records()
            .await?
            .into_iter()
            .map(|(_, record)| record.key)
            .collect::<Vec<_>>();
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
        canonical_path_for(&self.root, key)
    }

    fn prepare_publications(
        &self,
        publications: &[RuntimeSurfacePlanPublication],
    ) -> UseResult<Vec<(PathBuf, Vec<u8>, RuntimeSurfacePlanKey)>> {
        if publications.len() > MAX_RUNTIME_SURFACE_PLAN_RECORDS {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "The Runtime plan publication batch exceeds its record bound.",
            ));
        }
        let mut total_bytes = 0_usize;
        let mut prepared = Vec::with_capacity(publications.len());
        for publication in publications {
            self.validate_key_scope(&publication.key)?;
            validate_record(&publication.key, &publication.plan)?;
            let bytes = encode_record(&publication.key, &publication.plan)?;
            total_bytes = total_bytes.checked_add(bytes.len()).ok_or_else(|| {
                store_error(
                    PLAN_STORE_ERROR,
                    "The Runtime plan publication byte count overflowed.",
                )
            })?;
            if total_bytes > MAX_RUNTIME_SURFACE_PLAN_BATCH_BYTES {
                return Err(store_error(
                    PLAN_STORE_ERROR,
                    "The Runtime plan publication batch exceeds its byte bound.",
                ));
            }
            let path = self.path_for(&publication.key)?;
            prepared.push((path, bytes, publication.key.clone()));
        }
        prepared.sort_by(|left, right| left.0.cmp(&right.0));
        if prepared
            .windows(2)
            .any(|pair| pair[0].2 == pair[1].2 || pair[0].0 == pair[1].0)
        {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan publication batch contains duplicate key identities.",
            ));
        }
        Ok(prepared)
    }

    /// Scan and semantically verify every record beneath the owned root.
    ///
    /// Callers hold either the installation maintenance guard or the plan-store
    /// lock. The scan still permits bounded temporary files because a writer
    /// may have been interrupted after syncing its temporary bytes.
    async fn scan_records(&self) -> UseResult<Vec<(PathBuf, RuntimeSurfacePlanStoreRecord)>> {
        scan_records_at(&self.root, Some(&self.installation)).await
    }

    async fn acquire_lock(&self) -> UseResult<StdFile> {
        acquire_lock_for(&self.state_root, &self.root).await
    }
}

async fn scan_records_at(
    root: &Path,
    expected_installation: Option<&InstallationId>,
) -> UseResult<Vec<(PathBuf, RuntimeSurfacePlanStoreRecord)>> {
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read Runtime plan store", root, error))?;
    let mut records = Vec::new();
    let mut entries_seen = 0_usize;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read Runtime plan store entry", root, error))?
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
        let path = entry.path();
        let record = read_record_at(&path).await?.ok_or_else(|| {
            store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan record disappeared during inventory.",
            )
        })?;
        record.key.validate()?;
        if let Some(installation) = expected_installation {
            installation.ensure_same(&record.key.scope)?;
        }
        validate_record(&record.key, &record.plan)?;
        if canonical_path_for(root, &record.key)? != path {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan record is not stored at its canonical key path.",
            ));
        }
        records.push((path, record));
    }
    records.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(records)
}

fn canonical_path_for(root: &Path, key: &RuntimeSurfacePlanKey) -> UseResult<PathBuf> {
    let digest = key.descriptor_digest()?;
    let hex = digest
        .strip_prefix("sha256:")
        .ok_or_else(|| store_error(PLAN_STORE_ERROR, "A Runtime plan key digest is invalid."))?;
    Ok(root.join(format!("{hex}.json")))
}

async fn acquire_lock_for(state_root: &Path, root: &Path) -> UseResult<StdFile> {
    fs::create_dir_all(state_root)
        .await
        .map_err(|error| path_error("create Runtime plan state root", state_root, error))?;
    validate_directory(state_root).await?;
    ensure_owned_directory(state_root, root).await?;
    let path = root.join(PLAN_STORE_LOCK);
    let file = open_plan_lock(&path, true)?.ok_or_else(|| {
        store_error(
            PLAN_STORE_IO,
            "The Runtime plan store lock disappeared while it was opened.",
        )
    })?;
    lock_plan_file(file, &path, LockMode::Exclusive).await
}

/// Open and shared-lock an existing plan-store lock without creating any
/// filesystem entry.  A missing lock is valid for a restored owner root: the
/// first publisher will create the operational lock under global reference
/// admission, while a collector already holds the inverse boundary.
async fn acquire_existing_shared_lock_for(
    state_root: &Path,
    root: &Path,
) -> UseResult<Option<StdFile>> {
    validate_directory(state_root).await?;
    validate_directory(root).await?;
    let path = root.join(PLAN_STORE_LOCK);
    let Some(file) = open_plan_lock(&path, false)? else {
        return Ok(None);
    };
    lock_plan_file(file, &path, LockMode::Shared)
        .await
        .map(Some)
}

#[derive(Debug, Clone, Copy)]
enum LockMode {
    Shared,
    Exclusive,
}

async fn lock_plan_file(mut file: StdFile, path: &Path, mode: LockMode) -> UseResult<StdFile> {
    let deadline = tokio::time::Instant::now() + PLAN_STORE_LOCK_WAIT;
    loop {
        let attempt = tokio::task::spawn_blocking(move || {
            let result = match mode {
                LockMode::Shared => FileExt::try_lock_shared(&file),
                LockMode::Exclusive => FileExt::try_lock_exclusive(&file),
            };
            (file, result)
        })
        .await
        .map_err(|error| {
            store_error(
                PLAN_STORE_IO,
                format!("Failed to acquire the Runtime plan store lock: {error}"),
            )
        })?;
        let (returned, result) = attempt;
        match result {
            Ok(()) => return Ok(returned),
            Err(error) if plan_lock_is_contended(&error) => {
                file = returned;
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(store_error(
                        "use.plugin.runtime.plan_store_busy",
                        "Another process owns the Runtime plan store lock.",
                    ));
                }
                tokio::time::sleep(
                    PLAN_STORE_LOCK_RETRY_INTERVAL.min(deadline.saturating_duration_since(now)),
                )
                .await;
            }
            Err(error) => return Err(path_error("acquire Runtime plan store lock", path, error)),
        }
    }
}

fn open_plan_lock(path: &Path, create: bool) -> UseResult<Option<StdFile>> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => validate_plan_lock_metadata(path, &metadata)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(path_error("inspect Runtime plan store lock", path, error)),
    }
    let mut options = StdOpenOptions::new();
    options
        .create(create)
        .truncate(false)
        .read(true)
        .write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound && !create => return Ok(None),
        Err(error) => return Err(path_error("open Runtime plan store lock", path, error)),
    };
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| path_error("inspect Runtime plan store lock", path, error))?;
    validate_plan_lock_metadata(path, &metadata)?;
    Ok(Some(file))
}

fn validate_plan_lock_metadata(path: &Path, metadata: &std::fs::Metadata) -> UseResult<()> {
    if a3s_use_core::metadata_is_link_or_reparse_point(metadata) || !metadata.is_file() {
        return Err(store_error(
            PLAN_STORE_ERROR,
            format!(
                "The Runtime plan store lock '{}' is not an owned regular file.",
                path.display()
            ),
        ));
    }
    Ok(())
}

fn plan_lock_is_contended(error: &io::Error) -> bool {
    if error.kind() == io::ErrorKind::WouldBlock {
        return true;
    }
    #[cfg(windows)]
    {
        matches!(error.raw_os_error(), Some(32 | 33))
    }
    #[cfg(not(windows))]
    {
        false
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

fn decode_record(bytes: &[u8]) -> UseResult<RuntimeSurfacePlanStoreRecord> {
    if bytes.is_empty() || bytes.len() > MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record exceeds its size bound.",
        ));
    }
    let record: RuntimeSurfacePlanStoreRecord = serde_json::from_slice(bytes).map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!("A Runtime plan record is invalid JSON: {error}"),
        )
    })?;
    if record.schema != RUNTIME_SURFACE_PLAN_STORE_SCHEMA {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "The Runtime plan store record schema is unsupported.",
        ));
    }
    validate_record(&record.key, &record.plan)?;
    if encode_record(&record.key, &record.plan)? != bytes {
        return Err(store_error(
            PLAN_STORE_ERROR,
            "A Runtime plan record is not canonical JSON.",
        ));
    }
    Ok(record)
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
    decode_record(&bytes).map(Some).map_err(|error| {
        store_error(
            PLAN_STORE_ERROR,
            format!(
                "Runtime plan record '{}' failed validation: {}",
                path.display(),
                error.message
            ),
        )
    })
}

fn compare_existing(existing: &RuntimeSurfacePlanStoreRecord, requested: &[u8]) -> UseResult<()> {
    let existing = encode_record(&existing.key, &existing.plan)?;
    if existing == requested {
        Ok(())
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

async fn validate_exact_record_directory(root: &Path) -> UseResult<()> {
    if !validate_existing_directory(root).await? {
        return Ok(());
    }
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read exact Runtime plan candidate", root, error))?;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read exact Runtime plan candidate entry", root, error))?
    {
        let name = entry.file_name();
        let name = name.to_str().ok_or_else(|| {
            store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan candidate filename is not UTF-8.",
            )
        })?;
        if !is_record_name(name) {
            return Err(store_error(
                PLAN_STORE_ERROR,
                "A Runtime plan candidate contains an operational or unknown entry.",
            ));
        }
        validate_regular_file(&entry.path()).await?;
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
