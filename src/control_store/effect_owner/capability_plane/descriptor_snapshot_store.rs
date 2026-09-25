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

#[path = "descriptor_snapshot_store_io.rs"]
mod store_io;
pub(crate) use store_io::*;

#[path = "descriptor_snapshot_restore.rs"]
pub(in crate::control_store) mod restore;
#[path = "descriptor_snapshot_retention.rs"]
pub(in crate::control_store) mod retention;

pub(super) const SNAPSHOT_DIRECTORY: &str = "capability-gateway/descriptor-snapshots";
pub(super) const SNAPSHOT_LOCK: &str = ".mutation.lock";
pub(super) const SNAPSHOT_STAGING: &str = ".staging";
pub(super) const SNAPSHOT_RETENTION_JOURNAL: &str = ".retention.journal";
pub(super) const MAX_DIRECTORY_ENTRIES: usize =
    MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS.saturating_mul(2);
pub(super) const MAX_STAGING_BYTES: u64 = 64 * 1024 * 1024;
pub(super) const LOCK_WAIT: Duration = Duration::from_secs(2);
pub(super) const LOCK_RETRY: Duration = Duration::from_millis(25);

/// One exact content-addressed descriptor snapshot captured for Control
/// payload snapshot or restore materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotStoredRecord {
    pub digest: String,
    pub bytes: Vec<u8>,
}

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

    /// Capture every valid descriptor snapshot while an installation-wide
    /// exclusive maintenance fence is held.
    pub(in crate::control_store) async fn snapshot_records_under_maintenance(
        &self,
        maintenance: &a3s_use_extension::StateMaintenanceGuard,
    ) -> UseResult<Vec<ControlCapabilityDescriptorSnapshotStoredRecord>> {
        self.validate_configuration()?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(snapshot_error(
                "Descriptor snapshot capture requires the exact installation's exclusive maintenance guard.",
            ));
        }
        super::super::ensure_capability_payload_retention_quiescent(&self.state_root).await?;
        if !path_ancestors_exist(&self.state_root).await? {
            return Ok(Vec::new());
        }
        if !validate_existing_directory(&self.root).await? {
            return Ok(Vec::new());
        }
        retention::ensure_no_pending_journal(&self.root).await?;
        let mut records = Vec::new();
        for snapshot in scan_records(&self.root, &self.installation).await? {
            let bytes = encode_snapshot(&snapshot)?;
            let digest = snapshot.digest()?;
            records.push(ControlCapabilityDescriptorSnapshotStoredRecord { digest, bytes });
        }
        records.sort_by(|left, right| left.digest.cmp(&right.digest));
        Ok(records)
    }

    /// Inspect a restore candidate descriptor-snapshot directory.
    pub(in crate::control_store) async fn inspect_records_at(
        snapshots_root: &Path,
        installation: &InstallationId,
    ) -> UseResult<Vec<ControlCapabilityDescriptorSnapshotStoredRecord>> {
        installation.validate()?;
        if !validate_existing_directory(snapshots_root).await? {
            return Ok(Vec::new());
        }
        retention::ensure_no_pending_journal(snapshots_root).await?;
        let mut records = Vec::new();
        for snapshot in scan_records(snapshots_root, installation).await? {
            let bytes = encode_snapshot(&snapshot)?;
            let digest = snapshot.digest()?;
            records.push(ControlCapabilityDescriptorSnapshotStoredRecord { digest, bytes });
        }
        records.sort_by(|left, right| left.digest.cmp(&right.digest));
        Ok(records)
    }

    /// Materialize exact archived descriptor snapshots into a clean candidate
    /// directory under the complete restore exclusive fence.
    pub(in crate::control_store) async fn materialize_records(
        snapshots_root: &Path,
        installation: &InstallationId,
        records: &[ControlCapabilityDescriptorSnapshotStoredRecord],
    ) -> UseResult<()> {
        installation.validate()?;
        if records.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
            return Err(snapshot_error(
                "The descriptor snapshot restore source exceeds its record bound.",
            ));
        }
        ensure_directory_exists(snapshots_root).await?;
        retention::ensure_no_pending_journal(snapshots_root).await?;
        let existing = Self::inspect_records_at(snapshots_root, installation).await?;
        if !existing.is_empty() {
            if existing == records {
                return Ok(());
            }
            return Err(snapshot_error(
                "The descriptor snapshot restore candidate already contains different records.",
            ));
        }
        for record in records {
            let snapshot = decode_snapshot(&record.bytes)?;
            installation
                .ensure_same(&snapshot.key.installation)
                .map_err(|_| {
                    snapshot_error(
                        "An archived descriptor snapshot belongs to another installation.",
                    )
                })?;
            let digest = snapshot.digest()?;
            if digest != record.digest || encode_snapshot(&snapshot)? != record.bytes {
                return Err(snapshot_error(
                    "An archived descriptor snapshot digest differs from its bytes.",
                ));
            }
            let target = path_for_digest(snapshots_root, &digest)?;
            write_new_record(snapshots_root, &target, &record.bytes).await?;
        }
        let after = Self::inspect_records_at(snapshots_root, installation).await?;
        if after != records {
            return Err(snapshot_error(
                "The materialized descriptor snapshot candidate differs from its archive inventory.",
            ));
        }
        Ok(())
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
        super::super::ensure_capability_payload_retention_quiescent(&self.state_root).await?;
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
        super::super::ensure_capability_payload_retention_quiescent(&self.state_root).await?;
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
        super::super::ensure_capability_payload_retention_quiescent(&self.state_root).await?;
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
        super::super::ensure_capability_payload_retention_quiescent(&self.state_root).await?;
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

    /// Apply a reviewed retention plan while an outer coordinator owns the
    /// installation-wide maintenance fence.
    pub(in crate::control_store) async fn apply_retention_under_maintenance(
        &self,
        plan: &retention::ControlCapabilityDescriptorSnapshotRetentionPlan,
        expected_plan_digest: &str,
    ) -> UseResult<retention::ControlCapabilityDescriptorSnapshotRetentionResult> {
        retention::apply_retention_under_maintenance(self, plan, expected_plan_digest).await
    }

    /// Validate a reviewed retention target without unlinking any snapshot.
    pub(in crate::control_store) async fn ensure_retention_target_under_maintenance(
        &self,
        plan: &retention::ControlCapabilityDescriptorSnapshotRetentionPlan,
        expected_plan_digest: &str,
    ) -> UseResult<()> {
        retention::ensure_retention_target_under_maintenance(self, plan, expected_plan_digest).await
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

    /// Apply a reviewed descriptor restore while an outer coordinator owns
    /// the installation-wide maintenance fence.
    pub(in crate::control_store) async fn apply_clean_restore_under_maintenance(
        &self,
        plan: &restore::ControlCapabilityDescriptorSnapshotRestorePlan,
        snapshots: &[ControlCapabilityDescriptorSnapshot],
        plan_digest: &str,
        verification: restore::ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
    ) -> UseResult<restore::ControlCapabilityDescriptorSnapshotRestoreResult> {
        restore::apply_clean_restore_under_maintenance(
            self,
            plan,
            snapshots,
            plan_digest,
            verification,
        )
        .await
    }

    /// Validate a reviewed descriptor source while an outer coordinator owns
    /// the installation-wide maintenance fence. No bytes are written.
    pub(in crate::control_store) fn validate_clean_restore_source_under_maintenance(
        &self,
        plan: &restore::ControlCapabilityDescriptorSnapshotRestorePlan,
        snapshots: &[ControlCapabilityDescriptorSnapshot],
        plan_digest: &str,
        verification: &restore::ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
    ) -> UseResult<()> {
        restore::validate_clean_restore_source_under_maintenance(
            self,
            plan,
            snapshots,
            plan_digest,
            verification,
        )
    }

    /// Preflight a reviewed descriptor restore while an outer coordinator
    /// owns the installation-wide maintenance fence. No bytes are written.
    pub(in crate::control_store) async fn ensure_clean_restore_target_under_maintenance(
        &self,
        plan: &restore::ControlCapabilityDescriptorSnapshotRestorePlan,
        plan_digest: &str,
    ) -> UseResult<()> {
        restore::ensure_clean_restore_target_under_maintenance(self, plan, plan_digest).await
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
