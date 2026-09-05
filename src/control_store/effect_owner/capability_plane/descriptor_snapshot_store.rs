use std::fs::{File as StdFile, OpenOptions as StdOpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use a3s_use_core::{InstallationId, SignedCapabilityDescription, UseError, UseResult};
use a3s_use_extension::{CapabilityDescriptionTrustStore, ExtensionPaths, StateMaintenanceLock};
use fs2::FileExt;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::*;

#[path = "descriptor_snapshot_restore.rs"]
pub(in crate::control_store) mod restore;
#[path = "descriptor_snapshot_retention.rs"]
pub(in crate::control_store) mod retention;

const SNAPSHOT_DIRECTORY: &str = "capability-gateway/descriptor-snapshots";
const SNAPSHOT_LOCK: &str = ".mutation.lock";
const SNAPSHOT_STAGING: &str = ".staging";
pub(super) const SNAPSHOT_RETENTION_JOURNAL: &str = ".retention.journal";
const MAX_DIRECTORY_ENTRIES: usize =
    MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS.saturating_mul(2);
const MAX_STAGING_BYTES: u64 = 64 * 1024 * 1024;
const LOCK_WAIT: Duration = Duration::from_secs(2);
const LOCK_RETRY: Duration = Duration::from_millis(25);

/// Installation-scoped owner for immutable descriptor proof snapshots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotStore {
    installation: InstallationId,
    state_root: PathBuf,
    root: PathBuf,
}

impl ControlCapabilityDescriptorSnapshotStore {
    #[allow(dead_code)]
    pub(in crate::control_store) fn new(
        state_root: impl Into<PathBuf>,
        installation: InstallationId,
    ) -> UseResult<Self> {
        let state_root = state_root.into();
        let store = Self {
            root: state_root.join(SNAPSHOT_DIRECTORY),
            state_root,
            installation,
        };
        store.validate_configuration()?;
        Ok(store)
    }

    pub(in crate::control_store) fn from_extension_paths(paths: &ExtensionPaths) -> Self {
        let state_root = paths.installation_state_root();
        Self {
            installation: paths.installation().clone(),
            root: state_root.join(SNAPSHOT_DIRECTORY),
            state_root,
        }
    }

    #[allow(dead_code)]
    pub(in crate::control_store) fn installation(&self) -> &InstallationId {
        &self.installation
    }

    #[allow(dead_code)]
    pub(in crate::control_store) fn root(&self) -> &Path {
        &self.root
    }

    #[allow(dead_code)]
    pub(in crate::control_store) fn state_root(&self) -> &Path {
        &self.state_root
    }

    pub(in crate::control_store) fn validate_configuration(&self) -> UseResult<()> {
        self.installation
            .validate()
            .map_err(|_| snapshot_error("The descriptor snapshot installation is invalid."))?;
        if !self.state_root.is_absolute()
            || self
                .state_root
                .components()
                .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
        {
            return Err(snapshot_error(
                "The descriptor snapshot state root must be absolute and normalized.",
            ));
        }
        if self.root != self.state_root.join(SNAPSHOT_DIRECTORY) {
            return Err(snapshot_error(
                "The descriptor snapshot root is outside its installation state root.",
            ));
        }
        Ok(())
    }

    /// Publish one immutable proof snapshot. Equal canonical bytes are
    /// idempotent; a key can never be replaced with a different proof set or
    /// trust policy.
    pub(in crate::control_store) async fn publish(
        &self,
        snapshot: &ControlCapabilityDescriptorSnapshot,
    ) -> UseResult<ControlCapabilityDescriptorSnapshotPublication> {
        self.validate_configuration()?;
        snapshot.validate()?;
        self.installation
            .ensure_same(&snapshot.key.installation)
            .map_err(|_| {
                snapshot_error("The descriptor snapshot belongs to another installation.")
            })?;
        let bytes = encode_snapshot(snapshot)?;
        let key_digest = snapshot.key.digest()?;
        let snapshot_digest = snapshot.digest()?;
        let target = path_for_digest(&self.root, &snapshot_digest)?;

        ensure_directory_exists(&self.state_root).await?;
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        ensure_owned_directory_chain(&self.state_root, &self.root).await?;
        let _mutation = self.acquire_mutation().await?;
        retention::ensure_no_pending_journal(&self.root).await?;
        let records = scan_records(&self.root, &self.installation).await?;
        if let Some(current) = records.iter().find(|record| record.key == snapshot.key) {
            if current != snapshot {
                return Err(snapshot_conflict());
            }
            sync_directory(&self.root).await?;
            retire_staging(&self.root, &snapshot_digest).await?;
            return Ok(publication(snapshot, key_digest, snapshot_digest));
        }
        if records.len() >= MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
            return Err(snapshot_error(
                "The descriptor snapshot store reached its record bound.",
            ));
        }
        write_new_record(&self.root, &target, &bytes).await?;
        Ok(publication(snapshot, key_digest, snapshot_digest))
    }

    /// Verify and publish a signed-description snapshot in one explicit
    /// admission operation. The persisted record retains the canonical signed
    /// envelopes; the derived proof list is never accepted from the caller.
    pub(in crate::control_store) async fn publish_signed(
        &self,
        key: ControlCapabilityDescriptorSnapshotKey,
        signed_descriptions: Vec<SignedCapabilityDescription>,
        signer_policy: ControlCapabilitySignerPolicy,
        trust_store: &CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
    ) -> UseResult<ControlCapabilityDescriptorSnapshotPublication> {
        let snapshot = ControlCapabilityDescriptorSnapshot::new_signed(
            key,
            signed_descriptions,
            signer_policy,
            trust_store,
            now_unix_seconds,
        )?;
        self.publish(&snapshot).await
    }

    /// Read one exact snapshot. `None` means the key has not been published;
    /// malformed or substituted existing state is an error.
    pub(in crate::control_store) async fn get(
        &self,
        key: &ControlCapabilityDescriptorSnapshotKey,
    ) -> UseResult<Option<ControlCapabilityDescriptorSnapshot>> {
        self.validate_configuration()?;
        key.validate()?;
        self.installation
            .ensure_same(&key.installation)
            .map_err(|_| {
                snapshot_error("The requested descriptor snapshot belongs to another installation.")
            })?;
        if !path_ancestors_exist(&self.state_root).await? {
            return Ok(None);
        }
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        if !validate_existing_directory(&self.root).await? {
            return Ok(None);
        }
        let _lock = self.acquire_shared_lock().await?;
        retention::ensure_no_pending_journal(&self.root).await?;
        let records = scan_records(&self.root, &self.installation).await?;
        let mut matches = records.into_iter().filter(|record| record.key == *key);
        let Some(snapshot) = matches.next() else {
            return Ok(None);
        };
        if matches.next().is_some() {
            return Err(snapshot_conflict());
        }
        Ok(Some(snapshot))
    }

    /// Return every valid key in deterministic path order. This is an
    /// inspection primitive for the eventual backup/restore owner; it is not
    /// a source of desired-state authority.
    #[allow(dead_code)]
    pub(in crate::control_store) async fn keys(
        &self,
    ) -> UseResult<Vec<ControlCapabilityDescriptorSnapshotKey>> {
        self.validate_configuration()?;
        if !path_ancestors_exist(&self.state_root).await? {
            return Ok(Vec::new());
        }
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        if !validate_existing_directory(&self.root).await? {
            return Ok(Vec::new());
        }
        let _lock = self.acquire_shared_lock().await?;
        retention::ensure_no_pending_journal(&self.root).await?;
        let mut keys = scan_records(&self.root, &self.installation)
            .await?
            .into_iter()
            .map(|snapshot| snapshot.key)
            .collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }

    /// Build an exact, path-free retention plan for immutable descriptor
    /// snapshots. The caller supplies the digests that must survive; every
    /// other record is explicitly named for removal.
    pub(in crate::control_store) async fn plan_retention(
        &self,
        retain_digests: &[String],
    ) -> UseResult<retention::ControlCapabilityDescriptorSnapshotRetentionPlan> {
        self.validate_configuration()?;
        let retain_digests = retention::validate_requested_digests(retain_digests)?;
        if !path_ancestors_exist(&self.state_root).await? {
            return retention::build_plan(self.installation.clone(), Vec::new(), &retain_digests);
        }
        let _maintenance = StateMaintenanceLock::new(&self.state_root)
            .acquire_shared()
            .await?;
        if !validate_existing_directory(&self.root).await? {
            return retention::build_plan(self.installation.clone(), Vec::new(), &retain_digests);
        }
        let _lock = self.acquire_shared_lock().await?;
        retention::ensure_no_pending_journal(&self.root).await?;
        let records = scan_records(&self.root, &self.installation).await?;
        retention::build_plan(self.installation.clone(), records, &retain_digests)
    }

    /// Apply one reviewed descriptor-snapshot retention plan under the owner
    /// lock. Removal is journaled one record at a time so a restart can
    /// distinguish an unstarted unlink from an already completed unlink.
    pub(in crate::control_store) async fn apply_retention(
        &self,
        plan: &retention::ControlCapabilityDescriptorSnapshotRetentionPlan,
        expected_plan_digest: &str,
    ) -> UseResult<retention::ControlCapabilityDescriptorSnapshotRetentionResult> {
        retention::apply_retention(self, plan, expected_plan_digest).await
    }

    /// Resume the exact descriptor-snapshot retention operation left by a
    /// process interruption, if a durable owner journal is present.
    pub(in crate::control_store) async fn recover_retention(
        &self,
    ) -> UseResult<Option<retention::ControlCapabilityDescriptorSnapshotRetentionResult>> {
        retention::recover_retention(self).await
    }

    /// Build a path-free plan for restoring an exact descriptor-snapshot set
    /// into a clean owner target. The plan contains no source paths or trust
    /// decisions; apply revalidates both the canonical records and the
    /// current signed-description policy.
    pub(in crate::control_store) fn plan_clean_restore(
        &self,
        snapshots: &[ControlCapabilityDescriptorSnapshot],
    ) -> UseResult<restore::ControlCapabilityDescriptorSnapshotRestorePlan> {
        restore::plan_clean_restore(self, snapshots)
    }

    /// Apply one reviewed descriptor-snapshot restore only to a clean owner
    /// target. Signed v2 records require the explicit current trust policy
    /// verification mode before any candidate is published.
    pub(in crate::control_store) async fn apply_clean_restore(
        &self,
        plan: &restore::ControlCapabilityDescriptorSnapshotRestorePlan,
        snapshots: &[ControlCapabilityDescriptorSnapshot],
        expected_plan_digest: &str,
        verification: restore::ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
    ) -> UseResult<restore::ControlCapabilityDescriptorSnapshotRestoreResult> {
        restore::apply_clean_restore(self, plan, snapshots, expected_plan_digest, verification)
            .await
    }

    async fn acquire_mutation(&self) -> UseResult<SnapshotLock> {
        acquire_lock(&self.root, LockMode::Exclusive).await
    }

    async fn acquire_shared_lock(&self) -> UseResult<SnapshotLock> {
        acquire_lock(&self.root, LockMode::Shared).await
    }
}

fn publication(
    snapshot: &ControlCapabilityDescriptorSnapshot,
    key_digest: String,
    snapshot_digest: String,
) -> ControlCapabilityDescriptorSnapshotPublication {
    ControlCapabilityDescriptorSnapshotPublication {
        key: snapshot.key.clone(),
        key_digest,
        snapshot_digest,
        proof_set_digest: snapshot.proof_set_digest.clone(),
        signed_description_set_digest: snapshot.signed_description_set_digest().map(str::to_owned),
        signer_policy_digest: snapshot.signer_policy_digest.clone(),
    }
}

pub(super) fn encode_snapshot(
    snapshot: &ControlCapabilityDescriptorSnapshot,
) -> UseResult<Vec<u8>> {
    snapshot.validate()?;
    let record = SnapshotRecord::from(snapshot.clone());
    let bytes = canonical_json(&record, "descriptor proof snapshot")?;
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES {
        return Err(snapshot_error(
            "The descriptor proof snapshot exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

pub(super) fn decode_snapshot(bytes: &[u8]) -> UseResult<ControlCapabilityDescriptorSnapshot> {
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES {
        return Err(snapshot_conflict());
    }
    let record: SnapshotRecord = serde_json::from_slice(bytes).map_err(|_| snapshot_conflict())?;
    let snapshot =
        ControlCapabilityDescriptorSnapshot::try_from(record).map_err(|_| snapshot_conflict())?;
    if encode_snapshot(&snapshot)? != bytes {
        return Err(snapshot_conflict());
    }
    Ok(snapshot)
}

async fn read_snapshot_at(
    path: &Path,
    expected_snapshot_digest: &str,
) -> UseResult<Option<ControlCapabilityDescriptorSnapshot>> {
    let metadata = match fs::symlink_metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(path_error("inspect descriptor snapshot", path, error)),
    };
    if metadata_is_link(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() as usize > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES
    {
        return Err(snapshot_conflict());
    }
    let before = file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    configure_no_follow(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("open descriptor snapshot", path, error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| path_error("inspect opened descriptor snapshot", path, error))?;
    if metadata_is_link(&opened)
        || !opened.is_file()
        || opened.len() != metadata.len()
        || file_identity(&opened) != before
    {
        return Err(snapshot_conflict());
    }
    let mut bytes = Vec::with_capacity(opened.len() as usize);
    (&mut file)
        .take((MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| path_error("read descriptor snapshot", path, error))?;
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("reinspect descriptor snapshot", path, error))?;
    if metadata_is_link(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || bytes.len() as u64 != opened.len()
    {
        return Err(snapshot_conflict());
    }
    let snapshot = decode_snapshot(&bytes)?;
    if snapshot.digest()? != expected_snapshot_digest {
        return Err(snapshot_conflict());
    }
    Ok(Some(snapshot))
}

async fn write_new_record(root: &Path, target: &Path, bytes: &[u8]) -> UseResult<()> {
    let staging = root.join(SNAPSHOT_STAGING);
    ensure_owned_directory_chain(root, &staging).await?;
    let key_digest = target
        .file_stem()
        .and_then(|value| value.to_str())
        .ok_or_else(path_invalid)?;
    let temporary = staging.join(format!(".{key_digest}.tmp"));
    prepare_staging_file(&temporary, bytes).await?;
    sync_directory(&staging).await?;
    match fs::hard_link(&temporary, target).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let Some(current) = read_snapshot_at(target, &format!("sha256:{key_digest}")).await?
            else {
                return Err(snapshot_conflict());
            };
            if encode_snapshot(&current)? != bytes {
                return Err(snapshot_conflict());
            }
            sync_directory(root).await?;
            retire_staging(root, &format!("sha256:{key_digest}")).await?;
            return Ok(());
        }
        Err(error) => return Err(path_error("publish descriptor snapshot", target, error)),
    }
    let Some(current) = read_snapshot_at(target, &format!("sha256:{key_digest}")).await? else {
        return Err(snapshot_conflict());
    };
    if encode_snapshot(&current)? != bytes {
        return Err(snapshot_conflict());
    }
    sync_directory(root).await?;
    retire_staging(root, &format!("sha256:{key_digest}")).await
}

async fn prepare_staging_file(path: &Path, bytes: &[u8]) -> UseResult<()> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) => {
            if metadata_is_link(&metadata)
                || !metadata.is_file()
                || metadata.len() as usize > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES
            {
                return Err(snapshot_conflict());
            }
            let current = fs::read(path)
                .await
                .map_err(|error| path_error("read descriptor snapshot staging", path, error))?;
            if current != bytes {
                fs::remove_file(path).await.map_err(|error| {
                    path_error("retire descriptor snapshot staging", path, error)
                })?;
                sync_directory(path.parent().ok_or_else(path_invalid)?).await?;
            } else {
                return Ok(());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(path_error(
                "inspect descriptor snapshot staging",
                path,
                error,
            ))
        }
    }
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    configure_no_follow(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| path_error("create descriptor snapshot staging", path, error))?;
    if let Err(error) = async {
        file.write_all(bytes).await?;
        file.flush().await?;
        file.sync_all().await
    }
    .await
    {
        let _ = fs::remove_file(path).await;
        return Err(path_error("write descriptor snapshot staging", path, error));
    }
    drop(file);
    Ok(())
}

async fn retire_staging(root: &Path, key_digest: &str) -> UseResult<()> {
    let hex = key_digest
        .strip_prefix("sha256:")
        .ok_or_else(path_invalid)?;
    let path = root.join(SNAPSHOT_STAGING).join(format!(".{hex}.tmp"));
    match fs::symlink_metadata(&path).await {
        Ok(metadata) => {
            if metadata_is_link(&metadata) || !metadata.is_file() {
                return Err(path_invalid());
            }
            fs::remove_file(&path)
                .await
                .map_err(|error| path_error("retire descriptor snapshot staging", &path, error))?;
            sync_directory(path.parent().ok_or_else(path_invalid)?).await
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(path_error(
            "inspect descriptor snapshot staging",
            &path,
            error,
        )),
    }
}

async fn scan_records(
    root: &Path,
    installation: &InstallationId,
) -> UseResult<Vec<ControlCapabilityDescriptorSnapshot>> {
    let mut entries = fs::read_dir(root)
        .await
        .map_err(|error| path_error("read descriptor snapshot store", root, error))?;
    let mut count = 0_usize;
    let mut records = Vec::new();
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read descriptor snapshot entry", root, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_DIRECTORY_ENTRIES {
            return Err(snapshot_error(
                "The descriptor snapshot directory exceeds its bound.",
            ));
        }
        let name = entry
            .file_name()
            .to_str()
            .ok_or_else(|| snapshot_error("Descriptor snapshot names must be UTF-8."))?
            .to_owned();
        match name.as_str() {
            SNAPSHOT_LOCK => validate_regular_file(&entry.path()).await?,
            SNAPSHOT_RETENTION_JOURNAL => {
                retention::validate_journal_file(&entry.path()).await?;
            }
            SNAPSHOT_STAGING => validate_staging(&entry.path()).await?,
            _ if is_record_name(&name) => {
                let digest = format!("sha256:{}", name.trim_end_matches(".json"));
                let snapshot = read_snapshot_at(&entry.path(), &digest)
                    .await?
                    .ok_or_else(snapshot_conflict)?;
                installation
                    .ensure_same(&snapshot.key.installation)
                    .map_err(|_| {
                        snapshot_error("A descriptor snapshot belongs to another installation.")
                    })?;
                if snapshot.digest()? != digest {
                    return Err(snapshot_conflict());
                }
                records.push(snapshot);
            }
            _ => return Err(path_invalid()),
        }
    }
    if records.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
        return Err(snapshot_error(
            "The descriptor snapshot store exceeds its record bound.",
        ));
    }
    records.sort_by(|left, right| left.key.cmp(&right.key));
    Ok(records)
}

async fn validate_staging(path: &Path) -> UseResult<()> {
    validate_directory(path).await?;
    let mut entries = fs::read_dir(path)
        .await
        .map_err(|error| path_error("read descriptor snapshot staging", path, error))?;
    let mut count = 0_usize;
    let mut bytes = 0_u64;
    while let Some(entry) = entries
        .next_entry()
        .await
        .map_err(|error| path_error("read descriptor snapshot staging entry", path, error))?
    {
        count = count.saturating_add(1);
        if count > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
            return Err(snapshot_error(
                "Descriptor snapshot staging exceeds its entry bound.",
            ));
        }
        let file_name = entry.file_name();
        let name = file_name
            .to_str()
            .ok_or_else(|| snapshot_error("Descriptor snapshot staging names must be UTF-8."))?;
        let Some(hex) = name
            .strip_prefix('.')
            .and_then(|value| value.strip_suffix(".tmp"))
        else {
            return Err(path_invalid());
        };
        if hex.len() != 64
            || !hex
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        {
            return Err(path_invalid());
        }
        let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
            path_error("inspect descriptor snapshot staging", &entry.path(), error)
        })?;
        if metadata_is_link(&metadata)
            || !metadata.is_file()
            || metadata.len() as usize > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES
        {
            return Err(snapshot_conflict());
        }
        bytes = bytes
            .checked_add(metadata.len())
            .ok_or_else(|| snapshot_error("Descriptor snapshot staging size overflowed."))?;
        if bytes > MAX_STAGING_BYTES {
            return Err(snapshot_error(
                "Descriptor snapshot staging exceeds its byte bound.",
            ));
        }
    }
    Ok(())
}

fn is_record_name(name: &str) -> bool {
    let Some(hex) = name.strip_suffix(".json") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn path_for_digest(root: &Path, digest: &str) -> UseResult<PathBuf> {
    if !valid_sha256(digest) {
        return Err(path_invalid());
    }
    let hex = digest.strip_prefix("sha256:").ok_or_else(path_invalid)?;
    Ok(root.join(format!("{hex}.json")))
}

async fn acquire_lock(root: &Path, mode: LockMode) -> UseResult<SnapshotLock> {
    let path = root.join(SNAPSHOT_LOCK);
    let mut options = StdOpenOptions::new();
    options.create(true).read(true).write(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    let path_for_open = path.clone();
    let file = tokio::task::spawn_blocking(move || options.open(path_for_open))
        .await
        .map_err(|error| snapshot_io(format!("Descriptor snapshot lock task failed: {error}")))?
        .map_err(|error| path_error("open descriptor snapshot lock", &path, error))?;
    validate_regular_file(&path).await?;
    let deadline = tokio::time::Instant::now() + LOCK_WAIT;
    let mut file = file;
    loop {
        let (returned, result) = tokio::task::spawn_blocking(move || {
            let result = match mode {
                LockMode::Shared => FileExt::try_lock_shared(&file),
                LockMode::Exclusive => FileExt::try_lock_exclusive(&file),
            };
            (file, result)
        })
        .await
        .map_err(|error| snapshot_io(format!("Descriptor snapshot lock task failed: {error}")))?;
        file = returned;
        match result {
            Ok(()) => return Ok(SnapshotLock(file)),
            Err(error) if lock_contended(&error) => {
                let now = tokio::time::Instant::now();
                if now >= deadline {
                    return Err(UseError::new(
                        SNAPSHOT_BUSY,
                        "Another process owns the descriptor snapshot store lock.",
                    ));
                }
                tokio::time::sleep(LOCK_RETRY.min(deadline.saturating_duration_since(now))).await;
            }
            Err(error) => return Err(path_error("lock descriptor snapshot store", &path, error)),
        }
    }
}

#[derive(Debug)]
struct SnapshotLock(StdFile);

impl Drop for SnapshotLock {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}

#[derive(Debug, Clone, Copy)]
enum LockMode {
    Shared,
    Exclusive,
}

async fn ensure_directory_exists(path: &Path) -> UseResult<()> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut missing = Vec::new();
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link(&metadata) || !metadata.is_dir() {
                    return Err(path_invalid());
                }
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                missing.push(ancestor.to_path_buf())
            }
            Err(error) => {
                return Err(path_error(
                    "inspect descriptor snapshot root",
                    ancestor,
                    error,
                ))
            }
        }
    }
    while let Some(directory) = missing.pop() {
        match fs::create_dir(&directory).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(path_error(
                    "create descriptor snapshot directory",
                    &directory,
                    error,
                ))
            }
        }
        validate_directory(&directory).await?;
    }
    Ok(())
}

async fn ensure_owned_directory_chain(root: &Path, target: &Path) -> UseResult<()> {
    if !target.starts_with(root) {
        return Err(path_invalid());
    }
    ensure_directory_exists(root).await?;
    validate_directory(root).await?;
    let relative = target.strip_prefix(root).map_err(|_| path_invalid())?;
    let mut current = root.to_path_buf();
    for component in relative.components() {
        let Component::Normal(segment) = component else {
            return Err(path_invalid());
        };
        current.push(segment);
        match fs::symlink_metadata(&current).await {
            Ok(metadata) if !metadata_is_link(&metadata) && metadata.is_dir() => {}
            Ok(_) => return Err(path_invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current).await {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(path_error(
                            "create descriptor snapshot directory",
                            &current,
                            error,
                        ))
                    }
                }
                validate_directory(&current).await?;
            }
            Err(error) => {
                return Err(path_error(
                    "inspect descriptor snapshot directory",
                    &current,
                    error,
                ))
            }
        }
    }
    Ok(())
}

async fn path_ancestors_exist(path: &Path) -> UseResult<bool> {
    if !path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::CurDir | Component::ParentDir))
    {
        return Err(path_invalid());
    }
    let mut complete = true;
    for ancestor in path.ancestors() {
        if ancestor.as_os_str().is_empty() {
            continue;
        }
        match fs::symlink_metadata(ancestor).await {
            Ok(metadata) => {
                if metadata_is_link(&metadata) {
                    // A configured state root can be reached through an
                    // operating-system alias (for example macOS `/var`).
                    // Only a link at the configured path itself is invalid;
                    // aliases outside that ownership boundary are resolved
                    // and still required to denote directories.
                    if ancestor == path {
                        return Err(path_invalid());
                    }
                    let followed = fs::metadata(ancestor).await.map_err(|error| {
                        path_error(
                            "resolve descriptor snapshot state-root alias",
                            ancestor,
                            error,
                        )
                    })?;
                    if !followed.is_dir() {
                        return Err(path_invalid());
                    }
                } else if !metadata.is_dir() {
                    return Err(path_invalid());
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => complete = false,
            Err(error) => {
                return Err(path_error(
                    "inspect descriptor snapshot root",
                    ancestor,
                    error,
                ))
            }
        }
    }
    Ok(complete)
}

async fn validate_existing_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link(&metadata) && metadata.is_dir() => Ok(true),
        Ok(_) => Err(path_invalid()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(path_error(
            "inspect descriptor snapshot directory",
            path,
            error,
        )),
    }
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    if !validate_existing_directory(path).await? {
        return Err(path_invalid());
    }
    Ok(())
}

async fn validate_regular_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| path_error("inspect descriptor snapshot file", path, error))?;
    if metadata_is_link(&metadata) || !metadata.is_file() {
        return Err(path_invalid());
    }
    Ok(())
}

fn metadata_is_link(metadata: &std::fs::Metadata) -> bool {
    a3s_use_core::metadata_is_link_or_reparse_point(metadata)
}

fn configure_no_follow(options: &mut fs::OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;
        options
            .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FileIdentity {
    len: u64,
    modified: Option<std::time::SystemTime>,
    created: Option<std::time::SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn file_identity(metadata: &std::fs::Metadata) -> FileIdentity {
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    FileIdentity {
        len: metadata.len(),
        modified: metadata.modified().ok(),
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || {
        let file = StdOpenOptions::new()
            .read(true)
            .open(&path)
            .map_err(|error| {
                path_error("open descriptor snapshot directory for sync", &path, error)
            })?;
        file.sync_all()
            .map_err(|error| path_error("sync descriptor snapshot directory", &path, error))
    })
    .await
    .map_err(|error| {
        snapshot_io(format!(
            "Descriptor snapshot directory sync failed: {error}"
        ))
    })?
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}
