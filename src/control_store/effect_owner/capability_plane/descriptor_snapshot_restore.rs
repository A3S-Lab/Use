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
use tokio::fs as tokio_fs;
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

pub(super) const INVENTORY_DOMAIN: &[u8] =
    b"a3s.use.control-capability-descriptor-snapshot-restore-inventory.v1\0";
pub(super) const STAGING_PREFIX: &str = ".descriptor-snapshot-restore-";
pub(super) const CANDIDATE_DIRECTORY: &str = "candidate";
pub(super) const ACTIVATION_FILE: &str = "activation.json";
pub(super) const ACTIVATION_PARTIAL_FILE: &str = "activation.json.partial";
pub(super) const ACTIVATION_SCHEMA: &str =
    "a3s.use.control-capability-descriptor-snapshot-restore-activation.v1";
pub(super) const MAX_PLAN_BYTES: usize = 2 * 1024 * 1024;
pub(super) const MAX_ACTIVATION_BYTES: usize = 64 * 1024;
pub(super) const MAX_RESTORE_BYTES: u64 = MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64
    * MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS as u64;
pub(super) const ERROR_INVALID: &str = "use.control.capability_descriptor_snapshot_restore_invalid";
pub(super) const ERROR_TARGET_NOT_EMPTY: &str =
    "use.control.capability_descriptor_snapshot_restore_target_not_empty";

#[path = "descriptor_snapshot_restore/helpers.rs"]
mod helpers;
#[path = "descriptor_snapshot_restore/layout.rs"]
mod layout;
#[path = "descriptor_snapshot_restore/filesystem.rs"]
mod filesystem;

use helpers::*;
use filesystem::*;
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
    ensure_directory_exists(&store.state_root).await?;
    let _maintenance = StateMaintenanceLock::new(&store.state_root)
        .acquire_exclusive()
        .await?;
    apply_clean_restore_under_maintenance(store, plan, snapshots, &plan_digest, verification).await
}

/// Apply a previously reviewed descriptor restore while the caller owns the
/// installation-wide exclusive maintenance fence. The helper intentionally
/// does not acquire a second state lock so multiple payload owners can share
/// one preflight and publication fence.
pub(super) async fn apply_clean_restore_under_maintenance(
    store: &ControlCapabilityDescriptorSnapshotStore,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    snapshots: &[ControlCapabilityDescriptorSnapshot],
    plan_digest: &str,
    verification: ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
) -> UseResult<ControlCapabilityDescriptorSnapshotRestoreResult> {
    store.validate_configuration()?;
    plan.validate()?;
    if !valid_digest(plan_digest) {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan digest is invalid.",
        ));
    }
    if plan.installation != store.installation {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan belongs to another installation.",
        ));
    }
    if plan.descriptor_digest()? != plan_digest {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan digest differs from its payload.",
        ));
    }
    super::super::super::ensure_capability_payload_retention_quiescent(&store.state_root).await?;
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
    let parent = store.root.parent().ok_or_else(|| {
        restore_invalid("The descriptor snapshot restore target has no owned parent directory.")
    })?;
    ensure_owned_directory_chain(&store.state_root, parent).await?;
    let staging = staging_directory(parent, plan_digest)?;
    reject_foreign_staging(parent, &staging).await?;

    match inspect_live(store).await? {
        LiveSnapshotRoot::Absent => {}
        LiveSnapshotRoot::Owned(current) if current == plan.records => {
            retire_completed_staging(store, &staging, plan, plan_digest).await?;
            return restore_result(plan, plan_digest.to_owned(), false);
        }
        LiveSnapshotRoot::Owned(_) => return Err(restore_target_not_empty()),
    }
    if plan.records.is_empty() {
        reject_unexpected_staging(&staging).await?;
        return restore_result(plan, plan_digest.to_owned(), false);
    }

    prepare_staging(store, &staging, &prepared, plan, plan_digest).await?;
    let candidate = staging.join(CANDIDATE_DIRECTORY);
    validate_candidate(store, &candidate, &plan.records).await?;
    if !recover_activation_marker(&staging, plan, plan_digest).await? {
        create_activation_marker(&staging, plan, plan_digest).await?;
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
    retire_staging(&staging, plan, plan_digest).await?;
    restore_result(plan, plan_digest.to_owned(), true)
}

/// Validate the reviewed descriptor source while an outer coordinator owns
/// the installation-wide maintenance fence. Signed envelopes are reverified
/// here, but no candidate or marker is written.
pub(super) fn validate_clean_restore_source_under_maintenance(
    store: &ControlCapabilityDescriptorSnapshotStore,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    snapshots: &[ControlCapabilityDescriptorSnapshot],
    plan_digest: &str,
    verification: &ControlCapabilityDescriptorSnapshotRestoreVerification<'_>,
) -> UseResult<()> {
    store.validate_configuration()?;
    plan.validate()?;
    if !valid_digest(plan_digest) || plan.descriptor_digest()? != plan_digest {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan digest differs from its payload.",
        ));
    }
    if plan.installation != store.installation {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan belongs to another installation.",
        ));
    }
    let prepared = prepare_snapshots(store, snapshots, Some(verification))?;
    if prepared
        .iter()
        .map(|snapshot| &snapshot.entry)
        .ne(plan.records.iter())
    {
        return Err(restore_invalid(
            "The supplied descriptor snapshot set differs from the reviewed plan.",
        ));
    }
    Ok(())
}

/// Validate the descriptor owner target without writing payload bytes. This
/// is the coordinator's all-owner preflight step: any known conflict is
/// rejected before the first owner publishes.
pub(super) async fn ensure_clean_restore_target_under_maintenance(
    store: &ControlCapabilityDescriptorSnapshotStore,
    plan: &ControlCapabilityDescriptorSnapshotRestorePlan,
    plan_digest: &str,
) -> UseResult<()> {
    store.validate_configuration()?;
    plan.validate()?;
    if !valid_digest(plan_digest) || plan.descriptor_digest()? != plan_digest {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan digest differs from its payload.",
        ));
    }
    if plan.installation != store.installation {
        return Err(restore_invalid(
            "The descriptor snapshot restore plan belongs to another installation.",
        ));
    }
    ensure_directory_exists(&store.state_root).await?;
    let parent = store.root.parent().ok_or_else(|| {
        restore_invalid("The descriptor snapshot restore target has no owned parent directory.")
    })?;
    ensure_owned_directory_chain(&store.state_root, parent).await?;
    let staging = staging_directory(parent, plan_digest)?;
    reject_foreign_staging(parent, &staging).await?;
    match inspect_live(store).await? {
        LiveSnapshotRoot::Absent => {}
        LiveSnapshotRoot::Owned(current) if current == plan.records => {}
        LiveSnapshotRoot::Owned(_) => return Err(restore_target_not_empty()),
    }
    if plan.records.is_empty() {
        reject_unexpected_staging(&staging).await?;
    }
    Ok(())
}

#[derive(Debug)]
pub(super) struct PreparedSnapshot {
    pub(super) entry: ControlCapabilityDescriptorSnapshotRestoreEntry,
    pub(super) bytes: Vec<u8>,
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
