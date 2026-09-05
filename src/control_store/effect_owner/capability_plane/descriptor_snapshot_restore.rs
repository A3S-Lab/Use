//! Plan-bound clean-target restore for descriptor-snapshot payloads.
//!
//! A descriptor snapshot is immutable evidence, not lifecycle authority. The
//! restore owner therefore accepts an exact reviewed set, re-verifies signed
//! envelopes against the current trust policy, and publishes a complete owner
//! directory only when the target is clean. No existing snapshot is merged,
//! replaced, or silently selected as the current generation.

use std::io;
use std::path::{Path, PathBuf};

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{CapabilityDescriptionTrustStore, StateMaintenanceLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    canonical_json, encode_snapshot, ensure_directory_exists, ensure_owned_directory_chain,
    file_identity, metadata_is_link, path_for_digest, scan_records, sync_directory,
    validate_existing_directory, validate_regular_file, write_new_record,
    ControlCapabilityDescriptorSnapshot, ControlCapabilityDescriptorSnapshotStore,
    MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES,
    MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS, SNAPSHOT_STAGING,
};

/// Canonical schema for one reviewed descriptor-snapshot restore plan.
pub(in crate::control_store) const CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_PLAN_SCHEMA:
    &str = "a3s.use.control-capability-descriptor-snapshot-restore-plan.v1";
/// Canonical schema for one completed descriptor-snapshot restore.
pub(in crate::control_store) const CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_RESULT_SCHEMA:
    &str = "a3s.use.control-capability-descriptor-snapshot-restore-result.v1";

const INVENTORY_DOMAIN: &[u8] =
    b"a3s.use.control-capability-descriptor-snapshot-restore-inventory.v1\0";
const STAGING_PREFIX: &str = ".descriptor-snapshot-restore-";
const CANDIDATE_DIRECTORY: &str = "candidate";
const ACTIVATION_FILE: &str = "activation.json";
const ACTIVATION_PARTIAL_FILE: &str = "activation.json.partial";
const ACTIVATION_SCHEMA: &str =
    "a3s.use.control-capability-descriptor-snapshot-restore-activation.v1";
const MAX_PLAN_BYTES: usize = 2 * 1024 * 1024;
const MAX_ACTIVATION_BYTES: usize = 64 * 1024;
const MAX_RESTORE_BYTES: u64 = MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64
    * MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS as u64;
const ERROR_INVALID: &str = "use.control.capability_descriptor_snapshot_restore_invalid";
const ERROR_TARGET_NOT_EMPTY: &str =
    "use.control.capability_descriptor_snapshot_restore_target_not_empty";

#[path = "descriptor_snapshot_restore/layout.rs"]
mod layout;

use layout::{reject_foreign_staging, validate_candidate_layout, validate_restore_staging_layout};

/// One immutable descriptor snapshot named by a clean-target restore plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotRestoreEntry {
    pub(in crate::control_store) digest: String,
    pub(in crate::control_store) key_digest: String,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) capability_generation: u64,
    pub(in crate::control_store) byte_count: u64,
    pub(in crate::control_store) signed: bool,
}

impl ControlCapabilityDescriptorSnapshotRestoreEntry {
    fn from_snapshot(snapshot: &ControlCapabilityDescriptorSnapshot) -> UseResult<Self> {
        Ok(Self {
            digest: snapshot.digest()?,
            key_digest: snapshot.key.digest()?,
            installation_generation: snapshot.key.installation_generation,
            capability_generation: snapshot.key.capability_generation,
            byte_count: u64::try_from(encode_snapshot(snapshot)?.len()).map_err(|_| {
                restore_invalid(
                    "A descriptor snapshot restore byte count exceeds the platform range.",
                )
            })?,
            signed: snapshot.signed_descriptions.is_some(),
        })
    }

    fn validate(&self, installation: &InstallationId) -> UseResult<()> {
        installation.validate().map_err(|_| {
            restore_invalid("The descriptor snapshot restore installation is invalid.")
        })?;
        if !valid_digest(&self.digest)
            || !valid_digest(&self.key_digest)
            || self.installation_generation == 0
            || self.capability_generation == 0
            || self.byte_count == 0
            || self.byte_count > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64
        {
            return Err(restore_invalid(
                "A descriptor snapshot restore entry is invalid or exceeds its bounds.",
            ));
        }
        Ok(())
    }
}

/// Exact path-free record set approved for a clean descriptor restore.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotRestorePlan {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) record_count: u64,
    pub(in crate::control_store) byte_count: u64,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) records: Vec<ControlCapabilityDescriptorSnapshotRestoreEntry>,
}

impl ControlCapabilityDescriptorSnapshotRestorePlan {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            restore_invalid("The descriptor snapshot restore plan installation is invalid.")
        })?;
        if self.schema != CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_PLAN_SCHEMA
            || self.records.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS
            || self.record_count != u64::try_from(self.records.len()).unwrap_or(u64::MAX)
            || !valid_digest(&self.inventory_digest)
        {
            return Err(restore_invalid(
                "The descriptor snapshot restore plan identity or count is invalid.",
            ));
        }
        let mut total = 0_u64;
        let mut previous = None;
        for record in &self.records {
            record.validate(&self.installation)?;
            if previous.is_some_and(|digest| digest >= record.digest.as_str()) {
                return Err(restore_invalid(
                    "Descriptor snapshot restore records are duplicated or unordered.",
                ));
            }
            previous = Some(record.digest.as_str());
            total = total
                .checked_add(record.byte_count)
                .ok_or_else(|| restore_invalid("Descriptor snapshot restore bytes overflowed."))?;
        }
        if self.byte_count != total
            || total > MAX_RESTORE_BYTES
            || self.inventory_digest != inventory_digest(&self.records)?
        {
            return Err(restore_invalid(
                "The descriptor snapshot restore inventory accounting is invalid.",
            ));
        }
        let bytes = canonical_json(self, "descriptor snapshot restore plan")?;
        if bytes.is_empty() || bytes.len() > MAX_PLAN_BYTES {
            return Err(restore_invalid(
                "The descriptor snapshot restore plan exceeds its byte bound.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        Ok(digest(&canonical_json(
            self,
            "descriptor snapshot restore plan",
        )?))
    }
}

/// Bounded evidence returned after a descriptor restore completes or replays.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotRestoreResult {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) changed: bool,
    pub(in crate::control_store) restored_record_count: u64,
    pub(in crate::control_store) restored_byte_count: u64,
}

impl ControlCapabilityDescriptorSnapshotRestoreResult {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        self.installation.validate().map_err(|_| {
            restore_invalid("The descriptor snapshot restore result installation is invalid.")
        })?;
        if self.schema != CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_RESULT_SCHEMA
            || !valid_digest(&self.plan_digest)
            || !valid_digest(&self.inventory_digest)
            || self.restored_record_count
                > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS as u64
            || self.restored_byte_count > MAX_RESTORE_BYTES
            || (self.restored_record_count == 0 && self.restored_byte_count != 0)
        {
            return Err(restore_invalid(
                "The descriptor snapshot restore result identity or accounting is invalid.",
            ));
        }
        Ok(())
    }
}

/// Explicit replay policy for descriptor snapshots.
///
/// Signed v2 snapshots may only be restored after verification against the
/// current trust store and clock. Proof-only v1 snapshots remain an explicit
/// compatibility mode and never imply a cryptographic trust decision.
pub(in crate::control_store) enum ControlCapabilityDescriptorSnapshotRestoreVerification<'a> {
    ProofOnly,
    Signed {
        trust_store: &'a CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
    },
}

pub(super) fn plan_clean_restore(
    store: &ControlCapabilityDescriptorSnapshotStore,
    snapshots: &[ControlCapabilityDescriptorSnapshot],
) -> UseResult<ControlCapabilityDescriptorSnapshotRestorePlan> {
    store.validate_configuration()?;
    let prepared = prepare_snapshots(store, snapshots, None)?;
    let records = prepared
        .iter()
        .map(|snapshot| snapshot.entry.clone())
        .collect::<Vec<_>>();
    let byte_count = records.iter().try_fold(0_u64, |total, entry| {
        total
            .checked_add(entry.byte_count)
            .ok_or_else(|| restore_invalid("Descriptor snapshot restore bytes overflowed."))
    })?;
    let plan = ControlCapabilityDescriptorSnapshotRestorePlan {
        schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_PLAN_SCHEMA.to_owned(),
        installation: store.installation.clone(),
        record_count: u64::try_from(records.len())
            .map_err(|_| restore_invalid("Descriptor snapshot restore count overflowed."))?,
        byte_count,
        inventory_digest: inventory_digest(&records)?,
        records,
    };
    plan.validate()?;
    Ok(plan)
}

pub(super) async fn apply_clean_restore(
    store: &ControlCapabilityDescriptorSnapshotStore,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    snapshots: &[ControlCapabilityDescriptorSnapshot],
    expected_plan_digest: &str,
    verification: ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
) -> UseResult<ControlCapabilityDescriptorSnapshotRestoreResult> {
    store.validate_configuration()?;
    plan.validate()?;
    if !valid_digest(expected_plan_digest) {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan digest is invalid.",
        ));
    }
    if plan.installation != store.installation {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan belongs to another installation.",
        ));
    }
    let plan_digest = plan.descriptor_digest()?;
    if plan_digest != expected_plan_digest {
        return Err(restore_invalid(
            "The confirmed descriptor snapshot restore plan digest differs from its payload.",
        ));
    }
    let prepared = prepare_snapshots(store, snapshots, Some(&verification))?;
    if prepared
        .iter()
        .map(|snapshot| &snapshot.entry)
        .ne(plan.records.iter())
    {
        return Err(restore_invalid(
            "The supplied descriptor snapshot set differs from the reviewed plan.",
        ));
    }

    ensure_directory_exists(&store.state_root).await?;
    let _maintenance = StateMaintenanceLock::new(&store.state_root)
        .acquire_exclusive()
        .await?;
    ensure_directory_exists(&store.state_root).await?;
    let parent = store.root.parent().ok_or_else(|| {
        restore_invalid("The descriptor snapshot restore target has no owned parent directory.")
    })?;
    ensure_owned_directory_chain(&store.state_root, parent).await?;
    let staging = staging_directory(parent, &plan_digest)?;
    reject_foreign_staging(parent, &staging).await?;

    match inspect_live(store).await? {
        LiveSnapshotRoot::Absent => {}
        LiveSnapshotRoot::Owned(current) if current == plan.records => {
            retire_completed_staging(store, &staging, plan, &plan_digest).await?;
            return restore_result(plan, plan_digest, false);
        }
        LiveSnapshotRoot::Owned(_) => return Err(restore_target_not_empty()),
    }
    if plan.records.is_empty() {
        reject_unexpected_staging(&staging).await?;
        return restore_result(plan, plan_digest, false);
    }

    prepare_staging(store, &staging, &prepared, plan, &plan_digest).await?;
    let candidate = staging.join(CANDIDATE_DIRECTORY);
    validate_candidate(store, &candidate, &plan.records).await?;
    if !recover_activation_marker(&staging, plan, &plan_digest).await? {
        create_activation_marker(&staging, plan, &plan_digest).await?;
    }
    validate_candidate(store, &candidate, &plan.records).await?;
    if !matches!(inspect_live(store).await?, LiveSnapshotRoot::Absent) {
        return Err(restore_target_not_empty());
    }
    publish_candidate(candidate, store.root.clone()).await?;
    let LiveSnapshotRoot::Owned(current) = inspect_live(store).await? else {
        return Err(restore_invalid(
            "The activated descriptor snapshot owner directory is missing.",
        ));
    };
    if current != plan.records {
        return Err(restore_invalid(
            "The activated descriptor snapshot inventory differs from its plan.",
        ));
    }
    retire_staging(&staging, plan, &plan_digest).await?;
    restore_result(plan, plan_digest, true)
}

#[derive(Debug)]
struct PreparedSnapshot {
    entry: ControlCapabilityDescriptorSnapshotRestoreEntry,
    bytes: Vec<u8>,
}

fn prepare_snapshots(
    store: &ControlCapabilityDescriptorSnapshotStore,
    snapshots: &[ControlCapabilityDescriptorSnapshot],
    verification: Option<&ControlCapabilityDescriptorSnapshotRestoreVerification<'_>>,
) -> UseResult<Vec<PreparedSnapshot>> {
    if snapshots.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
        return Err(restore_invalid(
            "The descriptor snapshot restore source exceeds its record bound.",
        ));
    }
    let mut prepared = snapshots
        .iter()
        .map(|snapshot| {
            snapshot.validate().map_err(|error| {
                restore_invalid(format!(
                    "A descriptor snapshot restore source is invalid: {}",
                    error.message
                ))
            })?;
            if snapshot.key.installation != store.installation {
                return Err(restore_invalid(
                    "A descriptor snapshot restore source belongs to another installation.",
                ));
            }
            if let Some(verification) = verification {
                verify_snapshot(snapshot, verification)?;
            }
            let bytes = encode_snapshot(snapshot).map_err(|error| {
                restore_invalid(format!(
                    "A descriptor snapshot restore source is not canonical: {}",
                    error.message
                ))
            })?;
            let entry = ControlCapabilityDescriptorSnapshotRestoreEntry::from_snapshot(snapshot)?;
            entry.validate(&store.installation)?;
            Ok(PreparedSnapshot { entry, bytes })
        })
        .collect::<UseResult<Vec<_>>>()?;
    prepared.sort_by(|left, right| left.entry.digest.cmp(&right.entry.digest));
    if prepared
        .windows(2)
        .any(|pair| pair[0].entry.digest == pair[1].entry.digest)
    {
        return Err(restore_invalid(
            "The descriptor snapshot restore source contains duplicate records.",
        ));
    }
    let total = prepared.iter().try_fold(0_u64, |total, snapshot| {
        total
            .checked_add(snapshot.entry.byte_count)
            .ok_or_else(|| restore_invalid("Descriptor snapshot restore bytes overflowed."))
    })?;
    if total > MAX_RESTORE_BYTES {
        return Err(restore_invalid(
            "The descriptor snapshot restore source exceeds its byte bound.",
        ));
    }
    Ok(prepared)
}

fn verify_snapshot(
    snapshot: &ControlCapabilityDescriptorSnapshot,
    verification: &ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
) -> UseResult<()> {
    if snapshot.signed_descriptions.is_none() {
        return Ok(());
    }
    let ControlCapabilityDescriptorSnapshotRestoreVerification::Signed {
        trust_store,
        now_unix_seconds,
    } = verification
    else {
        return Err(restore_invalid(
            "A signed descriptor snapshot requires current trust-store verification.",
        ));
    };
    snapshot
        .reverify_signed(trust_store, *now_unix_seconds)
        .map_err(|error| {
            restore_invalid(format!(
                "Signed descriptor snapshot replay verification failed: {}",
                error.message
            ))
        })?;
    Ok(())
}

enum LiveSnapshotRoot {
    Absent,
    Owned(Vec<ControlCapabilityDescriptorSnapshotRestoreEntry>),
}

async fn inspect_live(
    store: &ControlCapabilityDescriptorSnapshotStore,
) -> UseResult<LiveSnapshotRoot> {
    if !validate_existing_directory(&store.root).await? {
        return Ok(LiveSnapshotRoot::Absent);
    }
    let records = scan_records(&store.root, &store.installation).await?;
    super::retention::ensure_no_pending_journal(&store.root).await?;
    if staged_records_present(&store.root).await? {
        return Err(restore_invalid(
            "The live descriptor snapshot owner contains residual staging evidence.",
        ));
    }
    let mut entries = records
        .iter()
        .map(ControlCapabilityDescriptorSnapshotRestoreEntry::from_snapshot)
        .collect::<UseResult<Vec<_>>>()?;
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    Ok(LiveSnapshotRoot::Owned(entries))
}

async fn staged_records_present(root: &Path) -> UseResult<bool> {
    let staging = root.join(SNAPSHOT_STAGING);
    if !validate_existing_directory(&staging).await? {
        return Ok(false);
    }
    let mut entries = fs::read_dir(&staging)
        .await
        .map_err(|error| restore_io("read descriptor snapshot staging", error))?;
    Ok(entries
        .next_entry()
        .await
        .map_err(|error| restore_io("inspect descriptor snapshot staging", error))?
        .is_some())
}

async fn prepare_staging(
    store: &ControlCapabilityDescriptorSnapshotStore,
    staging: &Path,
    records: &[PreparedSnapshot],
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    ensure_owned_directory_chain(&store.state_root, staging).await?;
    validate_restore_staging_layout(staging).await?;
    if recover_activation_marker(staging, plan, plan_digest).await? {
        let candidate = staging.join(CANDIDATE_DIRECTORY);
        if !validate_existing_directory(&candidate).await? {
            return Err(restore_invalid(
                "The descriptor snapshot restore candidate disappeared after activation began.",
            ));
        }
        return validate_candidate(store, &candidate, &plan.records).await;
    }
    let candidate = staging.join(CANDIDATE_DIRECTORY);
    if validate_existing_directory(&candidate).await? {
        if validate_candidate(store, &candidate, &plan.records)
            .await
            .is_ok()
        {
            return Ok(());
        }
        remove_candidate(staging, &candidate).await?;
    }
    ensure_owned_directory_chain(staging, &candidate).await?;
    for snapshot in records {
        let target = path_for_digest(&candidate, &snapshot.entry.digest)?;
        write_new_record(&candidate, &target, &snapshot.bytes).await?;
    }
    validate_candidate(store, &candidate, &plan.records).await
}

async fn validate_candidate(
    store: &ControlCapabilityDescriptorSnapshotStore,
    candidate: &Path,
    expected: &[ControlCapabilityDescriptorSnapshotRestoreEntry],
) -> UseResult<()> {
    validate_candidate_layout(candidate).await?;
    let records = scan_records(candidate, &store.installation).await?;
    let mut entries = records
        .iter()
        .map(ControlCapabilityDescriptorSnapshotRestoreEntry::from_snapshot)
        .collect::<UseResult<Vec<_>>>()?;
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    if entries != expected {
        return Err(restore_invalid(
            "The staged descriptor snapshot inventory differs from its plan.",
        ));
    }
    Ok(())
}

async fn remove_candidate(staging: &Path, candidate: &Path) -> UseResult<()> {
    if candidate.parent() != Some(staging)
        || candidate.file_name().and_then(|name| name.to_str()) != Some(CANDIDATE_DIRECTORY)
    {
        return Err(restore_invalid(
            "The descriptor snapshot restore candidate escapes its staging directory.",
        ));
    }
    let metadata = fs::symlink_metadata(candidate)
        .await
        .map_err(|error| restore_io("inspect descriptor snapshot restore candidate", error))?;
    if metadata_is_link(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "The descriptor snapshot restore candidate is not an owned directory.",
        ));
    }
    let candidate = candidate.to_path_buf();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::remove_dir_all_with_windows_retry_blocking(&candidate)
    })
    .await
    .map_err(|error| {
        restore_invalid(format!(
            "Descriptor snapshot candidate cleanup failed: {error}"
        ))
    })?
    .map_err(|error| restore_io("remove descriptor snapshot restore candidate", error))?;
    sync_directory(staging).await
}

async fn recover_activation_marker(
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<bool> {
    let expected = activation_bytes(plan, plan_digest)?;
    let marker = staging.join(ACTIVATION_FILE);
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker_length = optional_file_length(&marker).await?;
    let partial_length = optional_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The descriptor snapshot restore activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_exact_owned(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The descriptor snapshot restore activation marker differs from its plan.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        fs::remove_file(&partial)
            .await
            .map_err(|error| restore_io("remove incomplete descriptor restore marker", error))?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_exact_owned(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A complete descriptor restore marker partial has unexpected bytes.",
        ));
    }
    publish_noclobber(
        partial,
        marker,
        "publish descriptor restore activation marker",
    )
    .await?;
    sync_directory(staging).await?;
    Ok(true)
}

async fn create_activation_marker(
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    if recover_activation_marker(staging, plan, plan_digest).await? {
        return Ok(());
    }
    let bytes = activation_bytes(plan, plan_digest)?;
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker = staging.join(ACTIVATION_FILE);
    let mut options = fs::OpenOptions::new();
    options.create_new(true).write(true);
    super::configure_no_follow(&mut options);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options
        .open(&partial)
        .await
        .map_err(|error| restore_io("create descriptor restore activation marker", error))?;
    file.write_all(&bytes)
        .await
        .map_err(|error| restore_io("write descriptor restore activation marker", error))?;
    file.flush()
        .await
        .map_err(|error| restore_io("flush descriptor restore activation marker", error))?;
    file.sync_all()
        .await
        .map_err(|error| restore_io("sync descriptor restore activation marker", error))?;
    drop(file);
    if read_exact_owned(&partial, bytes.len() as u64).await? != bytes {
        return Err(restore_invalid(
            "The descriptor restore activation marker changed before publication.",
        ));
    }
    publish_noclobber(
        partial,
        marker,
        "publish descriptor restore activation marker",
    )
    .await?;
    sync_directory(staging).await
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Activation<'a> {
    schema: &'static str,
    installation: &'a InstallationId,
    plan_digest: &'a str,
    inventory_digest: &'a str,
    record_count: u64,
    byte_count: u64,
}

fn activation_bytes(
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<Vec<u8>> {
    plan.validate()?;
    if !valid_digest(plan_digest) || plan.descriptor_digest()? != plan_digest {
        return Err(restore_invalid(
            "The descriptor restore activation digest differs from its plan.",
        ));
    }
    let bytes = canonical_json(
        &Activation {
            schema: ACTIVATION_SCHEMA,
            installation: &plan.installation,
            plan_digest,
            inventory_digest: &plan.inventory_digest,
            record_count: plan.record_count,
            byte_count: plan.byte_count,
        },
        "descriptor snapshot restore activation",
    )?;
    if bytes.is_empty() || bytes.len() > MAX_ACTIVATION_BYTES {
        return Err(restore_invalid(
            "The descriptor restore activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

async fn publish_candidate(candidate: PathBuf, target: PathBuf) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_retain_blocking(candidate, &target)
    })
    .await
    .map_err(|error| {
        restore_invalid(format!(
            "Descriptor restore publication worker failed: {error}"
        ))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            restore_io(
                &format!(
                    "publish descriptor restore target '{}'",
                    error_target.display()
                ),
                error,
            )
        }
    })?;
    sync_directory(
        error_target.parent().ok_or_else(|| {
            restore_invalid("The descriptor restore target has no parent directory.")
        })?,
    )
    .await
}

async fn retire_completed_staging(
    store: &ControlCapabilityDescriptorSnapshotStore,
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    match fs::symlink_metadata(staging).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_io(
            "inspect completed descriptor restore staging",
            error,
        )),
        Ok(metadata) => {
            if metadata_is_link(&metadata) || !metadata.is_dir() {
                return Err(restore_invalid(
                    "The completed descriptor restore staging path is not an owned directory.",
                ));
            }
            validate_restore_staging_layout(staging).await?;
            let activation_started = recover_activation_marker(staging, plan, plan_digest).await?;
            let candidate = staging.join(CANDIDATE_DIRECTORY);
            let candidate_exists = validate_existing_directory(&candidate).await?;
            if candidate_exists {
                validate_candidate(store, &candidate, &plan.records).await?;
                remove_candidate(staging, &candidate).await?;
            }
            if activation_started {
                retire_staging(staging, plan, plan_digest).await
            } else if candidate_exists {
                retire_unmarked_staging(staging).await
            } else {
                Err(restore_invalid(
                    "The completed descriptor restore has ambiguous staged evidence.",
                ))
            }
        }
    }
}

async fn retire_staging(
    staging: &Path,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    validate_restore_staging_layout(staging).await?;
    if !recover_activation_marker(staging, plan, plan_digest).await?
        || validate_existing_directory(&staging.join(CANDIDATE_DIRECTORY)).await?
    {
        return Err(restore_invalid(
            "The descriptor restore staging directory cannot be retired before activation.",
        ));
    }
    let marker = staging.join(ACTIVATION_FILE);
    fs::remove_file(&marker)
        .await
        .map_err(|error| restore_io("retire descriptor restore activation marker", error))?;
    sync_directory(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read retired descriptor restore staging", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("finish descriptor restore staging", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The descriptor restore staging directory contains residual evidence.",
        ));
    }
    fs::remove_dir(staging)
        .await
        .map_err(|error| restore_io("retire descriptor restore staging", error))?;
    sync_directory(staging.parent().ok_or_else(|| {
        restore_invalid("The descriptor restore staging directory has no parent.")
    })?)
    .await
}

async fn retire_unmarked_staging(staging: &Path) -> UseResult<()> {
    validate_restore_staging_layout(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| restore_io("read unmarked descriptor restore staging", error))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| restore_io("finish unmarked descriptor restore staging", error))?
        .is_some()
    {
        return Err(restore_invalid(
            "The unmarked descriptor restore staging directory contains residual evidence.",
        ));
    }
    fs::remove_dir(staging)
        .await
        .map_err(|error| restore_io("retire unmarked descriptor restore staging", error))?;
    sync_directory(staging.parent().ok_or_else(|| {
        restore_invalid("The descriptor restore staging directory has no parent.")
    })?)
    .await
}

async fn reject_unexpected_staging(staging: &Path) -> UseResult<()> {
    match fs::symlink_metadata(staging).await {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(restore_io(
            "inspect empty descriptor restore staging",
            error,
        )),
        Ok(_) => Err(restore_invalid(
            "An empty descriptor restore plan has unexpected staged evidence.",
        )),
    }
}

async fn optional_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata) if !metadata_is_link(&metadata) && metadata.is_file() => {
            if metadata.len() == 0 || metadata.len() > MAX_ACTIVATION_BYTES as u64 {
                return Err(restore_invalid(
                    "A descriptor restore activation marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "A descriptor restore activation marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(restore_io(
            "inspect descriptor restore activation marker",
            error,
        )),
    }
}

async fn read_exact_owned(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    validate_regular_file(path).await?;
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("inspect descriptor restore activation marker", error))?;
    if metadata.len() != expected_length || expected_length > MAX_ACTIVATION_BYTES as u64 {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed before it was read.",
        ));
    }
    let before = file_identity(&metadata);
    let mut options = fs::OpenOptions::new();
    options.read(true);
    super::configure_no_follow(&mut options);
    let mut file = options
        .open(path)
        .await
        .map_err(|error| restore_io("open descriptor restore activation marker", error))?;
    let opened = file
        .metadata()
        .await
        .map_err(|error| restore_io("inspect opened descriptor restore marker", error))?;
    if !opened.is_file() || opened.len() != expected_length || file_identity(&opened) != before {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed while opened.",
        ));
    }
    let mut bytes = Vec::with_capacity(expected_length as usize);
    (&mut file)
        .take(expected_length.saturating_add(1))
        .read_to_end(&mut bytes)
        .await
        .map_err(|error| restore_io("read descriptor restore activation marker", error))?;
    if bytes.len() as u64 != expected_length {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed while read.",
        ));
    }
    let after = fs::symlink_metadata(path)
        .await
        .map_err(|error| restore_io("reinspect descriptor restore activation marker", error))?;
    if metadata_is_link(&after)
        || !after.is_file()
        || file_identity(&after) != before
        || after.len() != expected_length
    {
        return Err(restore_invalid(
            "A descriptor restore activation marker changed after it was read.",
        ));
    }
    Ok(bytes)
}

async fn publish_noclobber(source: PathBuf, target: PathBuf, action: &str) -> UseResult<()> {
    let error_target = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target)
    })
    .await
    .map_err(|error| restore_invalid(format!("Failed to join {action}: {error}")))?
    .map_err(|error| restore_io(&format!("{action} '{}'", error_target.display()), error))
}

fn staging_directory(parent: &Path, plan_digest: &str) -> UseResult<PathBuf> {
    let hex = plan_digest
        .strip_prefix("sha256:")
        .filter(|value| valid_hex(value, 64))
        .ok_or_else(|| restore_invalid("The descriptor restore plan digest is invalid."))?;
    Ok(parent.join(format!("{STAGING_PREFIX}{hex}")))
}

fn restore_result(
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: String,
    changed: bool,
) -> UseResult<ControlCapabilityDescriptorSnapshotRestoreResult> {
    let result = ControlCapabilityDescriptorSnapshotRestoreResult {
        schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RESTORE_RESULT_SCHEMA.to_owned(),
        installation: plan.installation.clone(),
        plan_digest,
        inventory_digest: plan.inventory_digest.clone(),
        changed,
        restored_record_count: plan.record_count,
        restored_byte_count: plan.byte_count,
    };
    result.validate()?;
    Ok(result)
}

fn inventory_digest(
    entries: &[ControlCapabilityDescriptorSnapshotRestoreEntry],
) -> UseResult<String> {
    let bytes = canonical_json(&entries, "descriptor snapshot restore inventory")?;
    let mut hasher = Sha256::new();
    hasher.update(INVENTORY_DOMAIN);
    hasher.update(bytes);
    Ok(format!("sha256:{:x}", hasher.finalize()))
}

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn valid_digest(value: &str) -> bool {
    value
        .strip_prefix("sha256:")
        .is_some_and(|hex| valid_hex(hex, 64))
}

fn valid_hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

fn restore_target_not_empty() -> UseError {
    UseError::new(
        ERROR_TARGET_NOT_EMPTY,
        "The clean-target descriptor snapshot restore refuses to merge or replace an existing owner directory.",
    )
}

fn restore_io(action: &str, error: io::Error) -> UseError {
    UseError::new(ERROR_INVALID, format!("Failed to {action}: {error}"))
}
