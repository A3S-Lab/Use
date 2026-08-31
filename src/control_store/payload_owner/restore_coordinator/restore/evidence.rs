use a3s_use_core::UseResult;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{
    canonical_json, coordinator_contract, restore_inventory_digest, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding, ControlRestoreCoordinatorEntry,
    ControlRestoreCoordinatorSnapshot, ControlRestoreCoordinatorState,
};
use super::restore_invalid;
use crate::control_store::model::valid_sha256;
use crate::state_restore::{
    StateRestoreHistorySnapshotActive, StateRestoreHistorySnapshotEntry,
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES,
};

const ACTIVATION_SCHEMA: &str = "a3s.use.control-restore-coordinator-activation.v1";
const ACTIVATION_DOMAIN: &[u8] = b"a3s.use.control-restore-coordinator-activation.v1\0";
const RESTORE_RESULT_SCHEMA: &str = "a3s.use.control-restore-coordinator-restore-result.v1";
const RESTORE_RESULT_DOMAIN: &[u8] = b"a3s.use.control-restore-coordinator-restore-result.v1\0";
pub(super) const MAX_ACTIVATION_BYTES: u64 = 128 * 1024;
const MAX_RESTORE_RESULT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct RestoreCoordinatorActivation {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    source_inventory_digest: String,
    pub(super) active_plan_digest: String,
    pub(super) active_marker_length: u64,
    pub(super) active_marker_sha256: String,
    pub(super) before_entries: Vec<ControlRestoreCoordinatorEntry>,
    before_inventory_digest: String,
    pub(super) target_entries: Vec<ControlRestoreCoordinatorEntry>,
    pub(super) target_inventory_digest: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pruned_source_plan_digest: Option<String>,
    descriptor_digest: String,
}

impl RestoreCoordinatorActivation {
    pub(super) fn new(
        snapshot: &ControlRestoreCoordinatorSnapshot,
        active: &StateRestoreHistorySnapshotActive,
        before_entries: Vec<ControlRestoreCoordinatorEntry>,
        target_entries: Vec<ControlRestoreCoordinatorEntry>,
        pruned_source_plan_digest: Option<String>,
    ) -> UseResult<Self> {
        let before_inventory_digest =
            restore_inventory_digest(&snapshot.manifest.binding.installation, &before_entries)?;
        let target_inventory_digest =
            restore_inventory_digest(&snapshot.manifest.binding.installation, &target_entries)?;
        let mut activation = Self {
            schema: ACTIVATION_SCHEMA.to_owned(),
            binding: snapshot.manifest.binding.clone(),
            owner_manifest_digest: snapshot.manifest.descriptor_digest.clone(),
            source_inventory_digest: snapshot.manifest.inventory_digest.clone(),
            active_plan_digest: active.plan_digest.clone(),
            active_marker_length: active.marker_length,
            active_marker_sha256: active.marker_sha256.clone(),
            before_entries,
            before_inventory_digest,
            target_entries,
            target_inventory_digest,
            pruned_source_plan_digest,
            descriptor_digest: String::new(),
        };
        activation.descriptor_digest = activation.expected_digest()?;
        Ok(activation)
    }

    pub(super) fn validate_for_snapshot(
        &self,
        snapshot: &ControlRestoreCoordinatorSnapshot,
        expected_target: &[ControlRestoreCoordinatorEntry],
        expected_pruned: Option<&str>,
    ) -> UseResult<()> {
        validate_entries(&self.before_entries)?;
        validate_entries(&self.target_entries)?;
        let source_count = snapshot.manifest.entries.len();
        let counts_valid = self.before_entries.len()
            <= STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES as usize
            && self.target_entries.len() <= source_count
            && source_count.saturating_sub(self.target_entries.len()) <= 1;
        if self.schema != ACTIVATION_SCHEMA
            || self.binding != snapshot.manifest.binding
            || self.owner_manifest_digest != snapshot.manifest.descriptor_digest
            || self.source_inventory_digest != snapshot.manifest.inventory_digest
            || !valid_sha256(&self.active_plan_digest)
            || self.active_marker_length == 0
            || !valid_sha256(&self.active_marker_sha256)
            || !counts_valid
            || self.target_entries != expected_target
            || self.pruned_source_plan_digest.as_deref() != expected_pruned
            || snapshot
                .manifest
                .entries
                .iter()
                .any(|entry| entry.plan_digest == self.active_plan_digest)
            || restore_inventory_digest(&self.binding.installation, &self.before_entries)?
                != self.before_inventory_digest
            || restore_inventory_digest(&self.binding.installation, &self.target_entries)?
                != self.target_inventory_digest
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_invalid(
                "The Restore Coordinator activation marker is invalid or was rebound.",
            ));
        }
        let bytes = self.canonical_bytes()?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_ACTIVATION_BYTES {
            return Err(restore_invalid(
                "The Restore Coordinator activation marker exceeds its byte bound.",
            ));
        }
        Ok(())
    }

    pub(super) fn binds_active(&self, active: &StateRestoreHistorySnapshotActive) -> bool {
        self.active_plan_digest == active.plan_digest
            && self.active_marker_length == active.marker_length
            && self.active_marker_sha256 == active.marker_sha256
    }

    pub(super) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        canonical_json(self).map_err(|error| {
            restore_invalid(format!(
                "Failed to encode Restore Coordinator activation: {error}"
            ))
        })
    }

    pub(super) fn decode_canonical(bytes: &[u8]) -> UseResult<Self> {
        let activation: Self = serde_json::from_slice(bytes).map_err(|_| {
            restore_invalid("The Restore Coordinator activation marker is invalid JSON.")
        })?;
        if activation.canonical_bytes()? != bytes {
            return Err(restore_invalid(
                "The Restore Coordinator activation marker is not canonically encoded.",
            ));
        }
        Ok(activation)
    }

    fn expected_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            owner_manifest_digest: &'a str,
            source_inventory_digest: &'a str,
            active_plan_digest: &'a str,
            active_marker_length: u64,
            active_marker_sha256: &'a str,
            before_entries: &'a [ControlRestoreCoordinatorEntry],
            before_inventory_digest: &'a str,
            target_entries: &'a [ControlRestoreCoordinatorEntry],
            target_inventory_digest: &'a str,
            pruned_source_plan_digest: Option<&'a str>,
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            owner_manifest_digest: &self.owner_manifest_digest,
            source_inventory_digest: &self.source_inventory_digest,
            active_plan_digest: &self.active_plan_digest,
            active_marker_length: self.active_marker_length,
            active_marker_sha256: &self.active_marker_sha256,
            before_entries: &self.before_entries,
            before_inventory_digest: &self.before_inventory_digest,
            target_entries: &self.target_entries,
            target_inventory_digest: &self.target_inventory_digest,
            pruned_source_plan_digest: self.pruned_source_plan_digest.as_deref(),
        })
        .map_err(|error| restore_invalid(format!("Failed to encode activation: {error}")))?;
        let mut digest = Sha256::new();
        digest.update(ACTIVATION_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlRestoreCoordinatorRestoreState {
    Absent,
    Archive {
        source_terminal_records: u64,
        restored_terminal_records: u64,
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRestoreCoordinatorRestoreResult {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    source_inventory_digest: String,
    restored_inventory_digest: String,
    pub(in crate::control_store) active_plan_digest: String,
    active_marker_sha256: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(in crate::control_store) pruned_source_plan_digest: Option<String>,
    pub(in crate::control_store) payload: ControlRestoreCoordinatorRestoreState,
    descriptor_digest: String,
}

impl ControlRestoreCoordinatorRestoreResult {
    pub(super) fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlRestoreCoordinatorSnapshot,
        activation: &RestoreCoordinatorActivation,
    ) -> UseResult<Self> {
        let payload = match &snapshot.manifest.payload {
            ControlRestoreCoordinatorState::Absent => ControlRestoreCoordinatorRestoreState::Absent,
            ControlRestoreCoordinatorState::Archive {
                archive_bytes,
                archive_sha256,
            } => ControlRestoreCoordinatorRestoreState::Archive {
                source_terminal_records: snapshot.manifest.entries.len() as u64,
                restored_terminal_records: activation.target_entries.len() as u64,
                archive_bytes: *archive_bytes,
                archive_sha256: archive_sha256.clone(),
            },
        };
        let mut result = Self {
            schema: RESTORE_RESULT_SCHEMA.to_owned(),
            binding: snapshot.manifest.binding.clone(),
            owner_manifest_digest: snapshot.manifest.descriptor_digest.clone(),
            source_inventory_digest: snapshot.manifest.inventory_digest.clone(),
            restored_inventory_digest: activation.target_inventory_digest.clone(),
            active_plan_digest: activation.active_plan_digest.clone(),
            active_marker_sha256: activation.active_marker_sha256.clone(),
            pruned_source_plan_digest: activation.pruned_source_plan_digest.clone(),
            payload,
            descriptor_digest: String::new(),
        };
        result.descriptor_digest = result.expected_digest()?;
        result.validate_for_snapshot(registry, snapshot, activation)?;
        Ok(result)
    }

    fn validate_for_snapshot(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlRestoreCoordinatorSnapshot,
        activation: &RestoreCoordinatorActivation,
    ) -> UseResult<()> {
        let limits = coordinator_contract(registry)?;
        self.binding.validate(registry)?;
        let payload_matches = match (&self.payload, &snapshot.manifest.payload) {
            (
                ControlRestoreCoordinatorRestoreState::Absent,
                ControlRestoreCoordinatorState::Absent,
            ) => activation.target_entries.is_empty(),
            (
                ControlRestoreCoordinatorRestoreState::Archive {
                    source_terminal_records,
                    restored_terminal_records,
                    archive_bytes,
                    archive_sha256,
                },
                ControlRestoreCoordinatorState::Archive {
                    archive_bytes: expected_bytes,
                    archive_sha256: expected_sha256,
                },
            ) => {
                *source_terminal_records == snapshot.manifest.entries.len() as u64
                    && *restored_terminal_records == activation.target_entries.len() as u64
                    && *archive_bytes == *expected_bytes
                    && *archive_bytes <= limits.max_payload_bytes
                    && archive_sha256 == expected_sha256
            }
            _ => false,
        };
        if self.schema != RESTORE_RESULT_SCHEMA
            || self.binding != snapshot.manifest.binding
            || self.owner_manifest_digest != snapshot.manifest.descriptor_digest
            || self.source_inventory_digest != snapshot.manifest.inventory_digest
            || self.restored_inventory_digest != activation.target_inventory_digest
            || self.active_plan_digest != activation.active_plan_digest
            || self.active_marker_sha256 != activation.active_marker_sha256
            || self.pruned_source_plan_digest != activation.pruned_source_plan_digest
            || !payload_matches
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_invalid(
                "The Restore Coordinator restore result differs from its exact activation.",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            owner_manifest_digest: &'a str,
            source_inventory_digest: &'a str,
            restored_inventory_digest: &'a str,
            active_plan_digest: &'a str,
            active_marker_sha256: &'a str,
            pruned_source_plan_digest: Option<&'a str>,
            payload: &'a ControlRestoreCoordinatorRestoreState,
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            owner_manifest_digest: &self.owner_manifest_digest,
            source_inventory_digest: &self.source_inventory_digest,
            restored_inventory_digest: &self.restored_inventory_digest,
            active_plan_digest: &self.active_plan_digest,
            active_marker_sha256: &self.active_marker_sha256,
            pruned_source_plan_digest: self.pruned_source_plan_digest.as_deref(),
            payload: &self.payload,
        })
        .map_err(|error| restore_invalid(format!("Failed to encode restore result: {error}")))?;
        if bytes.is_empty() || bytes.len() > MAX_RESTORE_RESULT_BYTES {
            return Err(restore_invalid(
                "The Restore Coordinator restore result exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(RESTORE_RESULT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

pub(super) fn entries_from_scan(
    entries: &[StateRestoreHistorySnapshotEntry],
) -> Vec<ControlRestoreCoordinatorEntry> {
    entries
        .iter()
        .map(|entry| ControlRestoreCoordinatorEntry {
            plan_digest: entry.plan_digest.clone(),
            length: entry.length,
            sha256: entry.sha256.clone(),
        })
        .collect()
}

fn validate_entries(entries: &[ControlRestoreCoordinatorEntry]) -> UseResult<()> {
    let mut prior = None;
    for entry in entries {
        entry.validate()?;
        if prior.is_some_and(|value| value >= entry.plan_digest.as_str()) {
            return Err(restore_invalid(
                "Restore Coordinator activation entries are not uniquely ordered.",
            ));
        }
        prior = Some(entry.plan_digest.as_str());
    }
    Ok(())
}
