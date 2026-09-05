//! Plan-bound, crash-recoverable retention for descriptor snapshots.
//!
//! Descriptor snapshots are immutable owner payloads, so deletion must be an
//! explicit reviewed partition rather than an age-based best effort.  The
//! journal records each unlink boundary and lets a restart distinguish an
//! already completed removal from one that still owns its target.

use std::collections::BTreeSet;
use std::path::Path;

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::StateMaintenanceLock;
use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::*;

#[path = "descriptor_snapshot_retention_journal.rs"]
mod journal;

use journal::RetentionJournal;

pub(in crate::control_store) const CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_PLAN_SCHEMA:
    &str = "a3s.use.control-capability-descriptor-snapshot-retention-plan.v1";
pub(in crate::control_store) const
    CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_RESULT_SCHEMA: &str =
    "a3s.use.control-capability-descriptor-snapshot-retention-result.v1";
pub(in crate::control_store) const
    CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_JOURNAL_SCHEMA: &str =
    "a3s.use.control-capability-descriptor-snapshot-retention-journal.v1";
const JOURNAL_SCHEMA: &str = CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_JOURNAL_SCHEMA;
const MAX_PLAN_BYTES: usize = 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 8 * 1024 * 1024;
const ERROR_INVALID: &str = "use.control.capability_descriptor_snapshot_retention_invalid";
const ERROR_STALE: &str = "use.control.capability_descriptor_snapshot_retention_stale";
const ERROR_OUTCOME_UNKNOWN: &str =
    "use.control.capability_descriptor_snapshot_retention_outcome_unknown";
const ERROR_JOURNAL_IO: &str = "use.control.capability_descriptor_snapshot_retention_journal_io";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotRetentionEntry {
    pub(in crate::control_store) digest: String,
    pub(in crate::control_store) key_digest: String,
    pub(in crate::control_store) installation_generation: u64,
    pub(in crate::control_store) capability_generation: u64,
    pub(in crate::control_store) byte_count: u64,
    pub(in crate::control_store) signed: bool,
}

impl ControlCapabilityDescriptorSnapshotRetentionEntry {
    fn from_snapshot(snapshot: &ControlCapabilityDescriptorSnapshot) -> UseResult<Self> {
        Ok(Self {
            digest: snapshot.digest()?,
            key_digest: snapshot.key.digest()?,
            installation_generation: snapshot.key.installation_generation,
            capability_generation: snapshot.key.capability_generation,
            byte_count: u64::try_from(encode_snapshot(snapshot)?.len()).map_err(|_| {
                retention_invalid("A descriptor snapshot byte count exceeds the platform range.")
            })?,
            signed: snapshot.signed_descriptions.is_some(),
        })
    }

    fn validate(&self, installation: &InstallationId) -> UseResult<()> {
        installation.validate()?;
        if !valid_digest(&self.digest)
            || !valid_digest(&self.key_digest)
            || self.installation_generation == 0
            || self.capability_generation == 0
            || self.byte_count == 0
            || self.byte_count > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64
        {
            return Err(retention_invalid(
                "A descriptor snapshot retention entry is invalid or exceeds its bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotRetentionPlan {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) before_record_count: u64,
    pub(in crate::control_store) before_inventory_digest: String,
    pub(in crate::control_store) remove: Vec<ControlCapabilityDescriptorSnapshotRetentionEntry>,
    pub(in crate::control_store) retain: Vec<ControlCapabilityDescriptorSnapshotRetentionEntry>,
}

impl ControlCapabilityDescriptorSnapshotRetentionPlan {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        if self.schema != CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_PLAN_SCHEMA
            || self.installation.validate().is_err()
            || !valid_digest(&self.before_inventory_digest)
        {
            return Err(retention_invalid(
                "The descriptor snapshot retention plan identity is invalid.",
            ));
        }
        let total = self.remove.len().saturating_add(self.retain.len());
        if self.before_record_count != u64::try_from(total).unwrap_or(u64::MAX)
            || total > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS
        {
            return Err(retention_invalid(
                "The descriptor snapshot retention plan count is invalid.",
            ));
        }
        if self.before_record_count > 0 && self.retain.is_empty() {
            return Err(retention_invalid(
                "A non-empty descriptor snapshot inventory must retain at least one record.",
            ));
        }
        validate_entries(&self.remove, &self.installation)?;
        validate_entries(&self.retain, &self.installation)?;
        if self.remove.iter().any(|entry| {
            self.retain
                .iter()
                .any(|retained| retained.digest == entry.digest)
        }) {
            return Err(retention_invalid(
                "The descriptor snapshot retention plan removes and retains the same digest.",
            ));
        }
        let mut all = self.remove.clone();
        all.extend(self.retain.clone());
        all.sort_by(|left, right| left.digest.cmp(&right.digest));
        if !all.windows(2).all(|pair| pair[0].digest < pair[1].digest) {
            return Err(retention_invalid(
                "The descriptor snapshot retention plan contains duplicate digests.",
            ));
        }
        if inventory_digest(&all)? != self.before_inventory_digest {
            return Err(retention_invalid(
                "The descriptor snapshot retention inventory digest is stale.",
            ));
        }
        canonical_bytes(self)?;
        Ok(())
    }

    pub(in crate::control_store) fn descriptor_digest(&self) -> UseResult<String> {
        self.validate()?;
        canonical_digest(self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityDescriptorSnapshotRetentionResult {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) changed: bool,
    pub(in crate::control_store) removed: Vec<ControlCapabilityDescriptorSnapshotRetentionEntry>,
    pub(in crate::control_store) retained_record_count: u64,
}

impl ControlCapabilityDescriptorSnapshotRetentionResult {
    pub(in crate::control_store) fn validate(&self) -> UseResult<()> {
        if self.schema != CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_RESULT_SCHEMA
            || self.installation.validate().is_err()
            || !valid_digest(&self.plan_digest)
            || self.removed.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS
            || self.retained_record_count
                > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS as u64
            || self.changed != !self.removed.is_empty()
            || self
                .retained_record_count
                .saturating_add(u64::try_from(self.removed.len()).unwrap_or(u64::MAX))
                > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS as u64
        {
            return Err(retention_invalid(
                "The descriptor snapshot retention result is invalid.",
            ));
        }
        validate_entries(&self.removed, &self.installation)?;
        canonical_bytes(self)?;
        Ok(())
    }
}

pub(super) fn validate_requested_digests(digests: &[String]) -> UseResult<BTreeSet<String>> {
    if digests.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
        return Err(retention_invalid(
            "The protected descriptor snapshot digest set exceeds its bound.",
        ));
    }
    let mut result = BTreeSet::new();
    for digest in digests {
        if !valid_digest(digest) || !result.insert(digest.clone()) {
            return Err(retention_invalid(
                "The protected descriptor snapshot digest set is invalid or duplicated.",
            ));
        }
    }
    Ok(result)
}

pub(super) fn build_plan(
    installation: InstallationId,
    snapshots: Vec<ControlCapabilityDescriptorSnapshot>,
    retain_digests: &BTreeSet<String>,
) -> UseResult<ControlCapabilityDescriptorSnapshotRetentionPlan> {
    let entries = entries_from_snapshots(&snapshots, &installation)?;
    if !entries.is_empty() && retain_digests.is_empty() {
        return Err(retention_invalid(
            "Retention requires at least one protected descriptor snapshot digest for a non-empty inventory.",
        ));
    }
    let known = entries
        .iter()
        .map(|entry| entry.digest.as_str())
        .collect::<BTreeSet<_>>();
    if retain_digests
        .iter()
        .any(|digest| !known.contains(digest.as_str()))
    {
        return Err(retention_stale(
            "A requested protected descriptor snapshot digest is not present in the inventory.",
        ));
    }
    let (retain, remove): (Vec<_>, Vec<_>) = entries
        .into_iter()
        .partition(|entry| retain_digests.contains(&entry.digest));
    let before_record_count = u64::try_from(retain.len().saturating_add(remove.len()))
        .map_err(|_| retention_invalid("The descriptor snapshot count exceeds its bound."))?;
    let plan = ControlCapabilityDescriptorSnapshotRetentionPlan {
        schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_PLAN_SCHEMA.to_owned(),
        installation,
        before_record_count,
        before_inventory_digest: inventory_digest_from_parts(&remove, &retain)?,
        remove,
        retain,
    };
    plan.validate()?;
    Ok(plan)
}

fn entries_from_snapshots(
    snapshots: &[ControlCapabilityDescriptorSnapshot],
    installation: &InstallationId,
) -> UseResult<Vec<ControlCapabilityDescriptorSnapshotRetentionEntry>> {
    if snapshots
        .iter()
        .any(|snapshot| snapshot.key.installation != *installation)
    {
        return Err(retention_stale(
            "A descriptor snapshot retention inventory contains a foreign installation.",
        ));
    }
    let mut entries = snapshots
        .iter()
        .map(ControlCapabilityDescriptorSnapshotRetentionEntry::from_snapshot)
        .collect::<UseResult<Vec<_>>>()?;
    entries.sort_by(|left, right| left.digest.cmp(&right.digest));
    if entries.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS {
        return Err(retention_invalid(
            "The descriptor snapshot inventory exceeds its record bound.",
        ));
    }
    validate_entries(&entries, installation)?;
    Ok(entries)
}

fn validate_entries(
    entries: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
    installation: &InstallationId,
) -> UseResult<()> {
    if entries.len() > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RECORDS
        || !entries
            .windows(2)
            .all(|pair| pair[0].digest < pair[1].digest)
    {
        return Err(retention_invalid(
            "The descriptor snapshot retention entries are not canonically ordered.",
        ));
    }
    for entry in entries {
        entry.validate(installation)?;
    }
    Ok(())
}

fn inventory_digest(
    entries: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
) -> UseResult<String> {
    let mut sorted = entries.to_vec();
    sorted.sort_by(|left, right| left.digest.cmp(&right.digest));
    canonical_digest(&sorted)
}

fn inventory_digest_from_parts(
    remove: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
    retain: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
) -> UseResult<String> {
    let mut all = remove.to_vec();
    all.extend(retain.iter().cloned());
    inventory_digest(&all)
}

fn canonical_digest<T: Serialize>(value: &T) -> UseResult<String> {
    let bytes = canonical_bytes(value)?;
    Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn canonical_bytes<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        retention_invalid(format!(
            "The descriptor snapshot retention value cannot be encoded: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() > MAX_PLAN_BYTES {
        return Err(retention_invalid(
            "The descriptor snapshot retention value exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

fn valid_digest(value: &str) -> bool {
    value.len() == 71
        && value.starts_with("sha256:")
        && value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

pub(super) async fn apply_retention(
    store: &ControlCapabilityDescriptorSnapshotStore,
    plan: &ControlCapabilityDescriptorSnapshotRetentionPlan,
    expected_plan_digest: &str,
) -> UseResult<ControlCapabilityDescriptorSnapshotRetentionResult> {
    store.validate_configuration()?;
    plan.validate()?;
    if plan.installation != store.installation {
        return Err(retention_stale(
            "The descriptor snapshot retention plan belongs to another installation.",
        ));
    }
    if !valid_digest(expected_plan_digest) || plan.descriptor_digest()? != expected_plan_digest {
        return Err(retention_stale(
            "The confirmed descriptor snapshot retention plan digest does not match its payload.",
        ));
    }

    let _maintenance = StateMaintenanceLock::new(&store.state_root)
        .acquire_shared()
        .await?;
    if !super::path_ancestors_exist(&store.state_root).await?
        || !super::validate_existing_directory(&store.root).await?
    {
        if plan.before_record_count == 0 && plan.retain.is_empty() {
            return retention_result(
                store.installation.clone(),
                expected_plan_digest.to_owned(),
                false,
                Vec::new(),
                0,
            );
        }
        return Err(retention_stale(
            "The descriptor snapshot state root disappeared after the retention plan was reviewed.",
        ));
    }

    let _mutation = store.acquire_mutation().await?;
    let records = super::scan_records(&store.root, &store.installation).await?;
    let current = entries_from_snapshots(&records, &store.installation)?;
    let mut journal = RetentionJournal::load(&store.root, plan, expected_plan_digest).await?;

    if journal.is_none() && current == plan.retain {
        return retention_result(
            store.installation.clone(),
            expected_plan_digest.to_owned(),
            false,
            Vec::new(),
            current.len(),
        );
    }
    if journal.is_none() {
        if inventory_digest(&current)? != plan.before_inventory_digest
            || current.len() != usize::try_from(plan.before_record_count).unwrap_or(usize::MAX)
            || !same_partition(&current, plan)
        {
            return Err(retention_stale(
                "The descriptor snapshot inventory changed after the retention plan was reviewed.",
            ));
        }
        journal = Some(RetentionJournal::create(&store.root, plan, expected_plan_digest).await?);
    }
    let mut journal = journal.ok_or_else(|| {
        retention_invalid("The descriptor snapshot retention journal was not initialized.")
    })?;

    loop {
        let records = super::scan_records(&store.root, &store.installation)
            .await
            .map_err(|error| {
                progress_error(
                    error,
                    &journal,
                    "Descriptor snapshot retention changed records, but the current inventory could not be read.",
                )
            })?;
        let current = entries_from_snapshots(&records, &store.installation).map_err(|error| {
            progress_error(
                error,
                &journal,
                "The descriptor snapshot retention recovery inventory could not be decoded.",
            )
        })?;
        if let Err(error) = reconcile_journal(&mut journal, &current).await {
            return Err(progress_error(
                error,
                &journal,
                "The descriptor snapshot retention journal does not match the inventory.",
            ));
        }
        if journal.is_completed() || journal.next_index().is_none() {
            if current != plan.retain {
                return Err(retention_outcome_unknown(
                    "The descriptor snapshot inventory is not the reviewed retained set after recovery.",
                    &journal.removed_entries(),
                ));
            }
            if !journal.is_completed() {
                if let Err(error) = journal.complete().await {
                    return Err(retention_outcome_unknown(
                        format!(
                            "Descriptor snapshot retention completed file removal, but its terminal checkpoint could not be persisted: {}",
                            error.message
                        ),
                        &journal.removed_entries(),
                    ));
                }
            }
            let removed = journal.removed_entries();
            if let Err(error) = journal.retire().await {
                return Err(retention_outcome_unknown(
                    format!(
                        "Descriptor snapshot retention completed, but its recovery journal could not be retired: {}",
                        error.message
                    ),
                    &removed,
                ));
            }
            return retention_result(
                store.installation.clone(),
                expected_plan_digest.to_owned(),
                !removed.is_empty(),
                removed,
                current.len(),
            );
        }

        let (index, checkpointed) = if let Some(index) = journal.in_flight() {
            (index, true)
        } else {
            (
                journal.next_index().ok_or_else(|| {
                    retention_invalid(
                        "The descriptor snapshot retention journal has no next removal.",
                    )
                })?,
                false,
            )
        };
        let entry = plan.remove.get(index).ok_or_else(|| {
            retention_invalid("The descriptor snapshot retention journal removal is out of bounds.")
        })?;
        let target = super::path_for_digest(&store.root, &entry.digest)?;
        verify_record_before_remove(&target, entry).await?;
        if !checkpointed {
            if let Err(error) = journal.begin_removal(index).await {
                return Err(retention_outcome_unknown(
                    format!(
                        "A descriptor snapshot removal was reviewed but its in-flight checkpoint could not be persisted: {}",
                        error.message
                    ),
                    &journal.removed_entries(),
                ));
            }
        }
        if let Err(error) = verify_record_before_remove(&target, entry).await {
            return Err(retention_outcome_unknown(
                format!(
                    "A reviewed descriptor snapshot changed after its in-flight checkpoint: {}",
                    error.message
                ),
                &journal.removed_entries(),
            ));
        }
        if let Err(error) = fs::remove_file(&target).await {
            return Err(retention_outcome_unknown(
                format!("A reviewed descriptor snapshot could not be removed: {error}"),
                &journal.removed_entries(),
            ));
        }
        let parent = target.parent().ok_or_else(|| {
            retention_invalid("A descriptor snapshot record has no parent directory.")
        })?;
        if let Err(error) = super::sync_directory(parent).await {
            let mut removed = journal.removed_entries();
            removed.push(entry.clone());
            return Err(retention_outcome_unknown(
                format!(
                    "A descriptor snapshot was removed, but directory durability could not be confirmed: {}",
                    error.message
                ),
                &removed,
            ));
        }
        if let Err(error) = journal.mark_removed(index).await {
            let mut removed = journal.removed_entries();
            removed.push(entry.clone());
            return Err(retention_outcome_unknown(
                format!(
                    "A descriptor snapshot was removed, but its recovery checkpoint could not be persisted: {}",
                    error.message
                ),
                &removed,
            ));
        }
    }
}

pub(super) async fn recover_retention(
    store: &ControlCapabilityDescriptorSnapshotStore,
) -> UseResult<Option<ControlCapabilityDescriptorSnapshotRetentionResult>> {
    store.validate_configuration()?;
    let pending = {
        let _maintenance = StateMaintenanceLock::new(&store.state_root)
            .acquire_shared()
            .await?;
        if !super::path_ancestors_exist(&store.state_root).await?
            || !super::validate_existing_directory(&store.root).await?
        {
            None
        } else {
            let _mutation = store.acquire_mutation().await?;
            RetentionJournal::load_unbound(&store.root)
                .await?
                .map(|journal| (journal.plan().clone(), journal.plan_digest().to_owned()))
        }
    };
    let Some((plan, digest)) = pending else {
        return Ok(None);
    };
    apply_retention(store, &plan, &digest).await.map(Some)
}

pub(super) async fn ensure_no_pending_journal(root: &Path) -> UseResult<()> {
    if RetentionJournal::has_pending(root).await? {
        return Err(retention_stale(
            "A descriptor snapshot retention recovery journal is pending; resume that exact plan before another mutation.",
        ));
    }
    Ok(())
}

pub(super) async fn validate_journal_file(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        retention_journal_io(format!(
            "Failed to inspect descriptor snapshot retention journal '{}': {error}",
            path.display()
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        return Err(retention_invalid(
            "The descriptor snapshot retention journal is not a bounded owned regular file.",
        ));
    }
    Ok(())
}

fn same_partition(
    current: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
    plan: &ControlCapabilityDescriptorSnapshotRetentionPlan,
) -> bool {
    let mut expected = plan.remove.clone();
    expected.extend(plan.retain.iter().cloned());
    expected.sort_by(|left, right| left.digest.cmp(&right.digest));
    expected == current
}

async fn verify_record_before_remove(
    path: &Path,
    expected: &ControlCapabilityDescriptorSnapshotRetentionEntry,
) -> UseResult<()> {
    let Some(snapshot) = super::read_snapshot_at(path, &expected.digest).await? else {
        return Err(retention_stale(
            "A reviewed descriptor snapshot disappeared before retention apply.",
        ));
    };
    let actual = ControlCapabilityDescriptorSnapshotRetentionEntry::from_snapshot(&snapshot)?;
    if actual != *expected {
        return Err(retention_stale(
            "A reviewed descriptor snapshot changed before retention apply.",
        ));
    }
    Ok(())
}

fn retention_result(
    installation: InstallationId,
    plan_digest: String,
    changed: bool,
    removed: Vec<ControlCapabilityDescriptorSnapshotRetentionEntry>,
    retained_record_count: usize,
) -> UseResult<ControlCapabilityDescriptorSnapshotRetentionResult> {
    let retained_record_count = u64::try_from(retained_record_count).map_err(|_| {
        retention_outcome_unknown(
            "The retained descriptor snapshot count exceeds the platform range.",
            &removed,
        )
    })?;
    let result = ControlCapabilityDescriptorSnapshotRetentionResult {
        schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_RESULT_SCHEMA.to_owned(),
        installation,
        plan_digest,
        changed,
        removed,
        retained_record_count,
    };
    result.validate()?;
    Ok(result)
}

async fn reconcile_journal(
    journal: &mut RetentionJournal,
    current: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
) -> UseResult<()> {
    if journal.is_completed() {
        if current != journal.plan().retain.as_slice() {
            return Err(retention_outcome_unknown(
                "A completed descriptor snapshot retention journal does not match the retained inventory.",
                &journal.removed_entries(),
            ));
        }
        return Ok(());
    }
    if let Some(index) = journal.in_flight() {
        if current == journal.expected_entries().as_slice() {
            return Ok(());
        }
        if let Some(after) = journal.expected_entries_after_in_flight() {
            if current == after.as_slice() {
                journal.mark_removed(index).await?;
                return Ok(());
            }
        }
        return Err(retention_outcome_unknown(
            "An in-flight descriptor snapshot removal has an unexpected inventory.",
            &journal.removed_entries(),
        ));
    }
    if current != journal.expected_entries().as_slice() {
        if journal.removed_count() == 0 {
            return Err(retention_stale(
                "The descriptor snapshot inventory changed before retention recovery resumed.",
            ));
        }
        return Err(retention_outcome_unknown(
            "The descriptor snapshot inventory changed during retention recovery.",
            &journal.removed_entries(),
        ));
    }
    Ok(())
}

fn progress_error(error: UseError, journal: &RetentionJournal, context: &str) -> UseError {
    if journal.removed_count() == 0 && journal.in_flight().is_none() {
        return error;
    }
    retention_outcome_unknown(
        format!("{context}: {}", error.message),
        &journal.removed_entries(),
    )
}

fn retention_invalid(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_INVALID, message)
}

fn retention_stale(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_STALE, message)
}

fn retention_outcome_unknown(
    message: impl Into<String>,
    removed: &[ControlCapabilityDescriptorSnapshotRetentionEntry],
) -> UseError {
    UseError::new(ERROR_OUTCOME_UNKNOWN, message).with_detail(
        "removedDigests",
        removed
            .iter()
            .map(|entry| entry.digest.clone())
            .collect::<Vec<_>>(),
    )
}

fn retention_journal_io(message: impl Into<String>) -> UseError {
    UseError::new(ERROR_JOURNAL_IO, message)
}

#[cfg(test)]
mod tests {
    use a3s_use_core::{InstallationKind, UseResult};
    use tokio::io::AsyncWriteExt;

    use super::*;

    fn installation() -> InstallationId {
        InstallationId::new(InstallationKind::Workspace, "descriptor-retention-tests").unwrap()
    }

    fn digest(seed: char) -> String {
        format!("sha256:{}", seed.to_string().repeat(64))
    }

    fn entry(seed: char, generation: u64) -> ControlCapabilityDescriptorSnapshotRetentionEntry {
        ControlCapabilityDescriptorSnapshotRetentionEntry {
            digest: digest(seed),
            key_digest: digest('c'),
            installation_generation: generation,
            capability_generation: generation,
            byte_count: 1,
            signed: false,
        }
    }

    fn plan() -> UseResult<ControlCapabilityDescriptorSnapshotRetentionPlan> {
        let remove = vec![entry('a', 1)];
        let retain = vec![entry('b', 2)];
        let mut all = remove.clone();
        all.extend(retain.clone());
        Ok(ControlCapabilityDescriptorSnapshotRetentionPlan {
            schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_PLAN_SCHEMA.to_owned(),
            installation: installation(),
            before_record_count: 2,
            before_inventory_digest: inventory_digest(&all)?,
            remove,
            retain,
        })
    }

    #[test]
    fn retention_plan_rejects_an_empty_protection_set_for_non_empty_inventory() {
        let removed = entry('a', 1);
        let plan = ControlCapabilityDescriptorSnapshotRetentionPlan {
            schema: CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_RETENTION_PLAN_SCHEMA.to_owned(),
            installation: installation(),
            before_record_count: 1,
            before_inventory_digest: inventory_digest(std::slice::from_ref(&removed)).unwrap(),
            remove: vec![removed],
            retain: Vec::new(),
        };
        let error = plan.validate().unwrap_err();
        assert_eq!(
            error.code,
            "use.control.capability_descriptor_snapshot_retention_invalid"
        );
    }

    #[tokio::test]
    async fn retention_journal_recovers_inflight_work_and_repairs_a_torn_tail() {
        let temporary = tempfile::tempdir().unwrap();
        let plan = plan().unwrap();
        let plan_digest = plan.descriptor_digest().unwrap();
        let mut journal = RetentionJournal::create(temporary.path(), &plan, &plan_digest)
            .await
            .unwrap();
        journal.begin_removal(0).await.unwrap();
        drop(journal);

        let path = temporary.path().join(SNAPSHOT_RETENTION_JOURNAL);
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .await
            .unwrap();
        file.write_all(br#"{"schema":"torn"}"#).await.unwrap();
        file.sync_all().await.unwrap();
        drop(file);

        let mut recovered = RetentionJournal::load_unbound(temporary.path())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.in_flight(), Some(0));
        let repaired = tokio::fs::read(&path).await.unwrap();
        assert!(repaired.ends_with(b"\n"));
        assert!(!repaired.windows(4).any(|window| window == b"torn"));

        recovered.mark_removed(0).await.unwrap();
        recovered.complete().await.unwrap();
        assert!(recovered.is_completed());
        recovered.retire().await.unwrap();
        assert!(!path.exists());
    }
}
