use std::collections::BTreeSet;
use std::path::PathBuf;

use a3s_use_core::{InstallationId, UseError, UseResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerLimits, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding, ControlPayloadSnapshotEvidence, ControlPayloadSnapshotReceipt,
    ControlPayloadSnapshotSession,
};
use crate::control_store::model::valid_sha256;
use crate::state_restore::{
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES,
    STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES,
};

mod archive;

pub(in crate::control_store) const CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-restore-coordinator-snapshot.v1";
const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-restore-coordinator-snapshot.v1\0";
const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-restore-coordinator-inventory.v1\0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRestoreCoordinatorEntry {
    pub(in crate::control_store) plan_digest: String,
    pub(in crate::control_store) length: u64,
    pub(in crate::control_store) sha256: String,
}

impl ControlRestoreCoordinatorEntry {
    fn validate(&self) -> UseResult<()> {
        if !valid_sha256(&self.plan_digest)
            || self.length == 0
            || self.length > STATE_RESTORE_HISTORY_SNAPSHOT_MAX_RECORD_BYTES
            || !valid_sha256(&self.sha256)
        {
            return Err(coordinator_error(
                "A Restore Coordinator entry is invalid or exceeds its byte bound.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlRestoreCoordinatorState {
    Absent,
    Archive {
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRestoreCoordinatorSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) payload: ControlRestoreCoordinatorState,
    pub(in crate::control_store) excluded_active_files: u64,
    pub(in crate::control_store) excluded_active_inventory_digest: String,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) entries: Vec<ControlRestoreCoordinatorEntry>,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlRestoreCoordinatorSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        created_at_ms: u64,
        payload: ControlRestoreCoordinatorState,
        excluded_active_files: u64,
        excluded_active_inventory_digest: String,
        entries: Vec<ControlRestoreCoordinatorEntry>,
    ) -> UseResult<Self> {
        let inventory_digest = restore_inventory_digest(&binding.installation, &entries)?;
        let mut manifest = Self {
            schema: CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA.to_owned(),
            binding,
            created_at_ms,
            payload,
            excluded_active_files,
            excluded_active_inventory_digest,
            inventory_digest,
            entries,
            descriptor_digest: String::new(),
        };
        manifest.descriptor_digest = manifest.expected_descriptor_digest()?;
        manifest.validate(registry, &manifest.binding)?;
        Ok(manifest)
    }

    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        let limits = coordinator_contract(registry)?;
        self.binding.validate(registry)?;
        if self.schema != CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA
            || &self.binding != expected_binding
            || self.created_at_ms == 0
            || !valid_sha256(&self.excluded_active_inventory_digest)
            || !valid_sha256(&self.inventory_digest)
            || !valid_sha256(&self.descriptor_digest)
        {
            return Err(coordinator_error(
                "The Restore Coordinator manifest is invalid or was rebound.",
            ));
        }

        let mut prior = None;
        let mut portable = BTreeSet::new();
        let mut byte_count = 0_u64;
        for entry in &self.entries {
            entry.validate()?;
            if prior.is_some_and(|prior| prior >= entry.plan_digest.as_str())
                || !portable.insert(entry.plan_digest.to_ascii_lowercase())
            {
                return Err(coordinator_error(
                    "Restore Coordinator entries are not uniquely ordered.",
                ));
            }
            byte_count = byte_count.checked_add(entry.length).ok_or_else(|| {
                coordinator_error("Restore Coordinator byte accounting overflowed.")
            })?;
            prior = Some(entry.plan_digest.as_str());
        }
        let file_count = u64::try_from(self.entries.len())
            .map_err(|_| coordinator_error("Restore Coordinator file accounting overflowed."))?;
        let scanned_files = file_count
            .checked_add(self.excluded_active_files)
            .ok_or_else(|| coordinator_error("Restore Coordinator file accounting overflowed."))?;
        let operation_files = file_count
            .checked_add(u64::from(self.excluded_active_files == 2))
            .ok_or_else(|| coordinator_error("Restore Coordinator file accounting overflowed."))?;
        if self.excluded_active_files > 2
            || operation_files > STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES
            || scanned_files > limits.max_files
            || byte_count > limits.max_payload_bytes
        {
            return Err(coordinator_error(
                "The Restore Coordinator payload exceeds its native or registered bounds.",
            ));
        }
        match &self.payload {
            ControlRestoreCoordinatorState::Absent if file_count == 0 && byte_count == 0 => {}
            ControlRestoreCoordinatorState::Archive {
                archive_bytes,
                archive_sha256,
            } if file_count > 0 && *archive_bytes == byte_count && valid_sha256(archive_sha256) => {
            }
            _ => {
                return Err(coordinator_error(
                    "Restore Coordinator archive evidence differs from its entries.",
                ))
            }
        }
        if restore_inventory_digest(&self.binding.installation, &self.entries)?
            != self.inventory_digest
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(coordinator_error(
                "The Restore Coordinator manifest digest is inconsistent.",
            ));
        }
        let bytes = canonical_json(self).map_err(|error| {
            coordinator_error(format!(
                "Failed to encode Restore Coordinator manifest: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() as u64 > limits.max_manifest_bytes {
            return Err(coordinator_error(
                "The Restore Coordinator manifest exceeds its registered byte bound.",
            ));
        }
        Ok(())
    }

    fn canonical_bytes(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<Vec<u8>> {
        self.validate(registry, expected_binding)?;
        canonical_json(self).map_err(|error| {
            coordinator_error(format!(
                "Failed to encode Restore Coordinator manifest: {error}"
            ))
        })
    }

    fn expected_descriptor_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            created_at_ms: u64,
            payload: &'a ControlRestoreCoordinatorState,
            excluded_active_files: u64,
            excluded_active_inventory_digest: &'a str,
            inventory_digest: &'a str,
            entries: &'a [ControlRestoreCoordinatorEntry],
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            created_at_ms: self.created_at_ms,
            payload: &self.payload,
            excluded_active_files: self.excluded_active_files,
            excluded_active_inventory_digest: &self.excluded_active_inventory_digest,
            inventory_digest: &self.inventory_digest,
            entries: &self.entries,
        })
        .map_err(|error| coordinator_error(format!("Failed to encode manifest: {error}")))?;
        let mut digest = Sha256::new();
        digest.update(SNAPSHOT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRestoreCoordinatorSnapshot {
    pub(in crate::control_store) manifest: ControlRestoreCoordinatorSnapshotManifest,
    pub(in crate::control_store) receipt: ControlPayloadSnapshotReceipt,
}

impl ControlRestoreCoordinatorSnapshot {
    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        self.manifest.validate(registry, expected_binding)?;
        self.receipt.validate(registry, expected_binding)?;
        let manifest_bytes = self.manifest.canonical_bytes(registry, expected_binding)?;
        let file_count = self.manifest.entries.len() as u64;
        let byte_count = self
            .manifest
            .entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length));
        let byte_count = byte_count
            .ok_or_else(|| coordinator_error("Restore Coordinator accounting overflowed."))?;
        if self.receipt.owner != ControlPayloadOwnerId::RestoreCoordinator
            || self.receipt.owner_manifest_digest != self.manifest.descriptor_digest
            || self.receipt.inventory_digest != self.manifest.inventory_digest
            || self.receipt.manifest_bytes != manifest_bytes.len() as u64
            || self.receipt.file_count != file_count
            || self.receipt.byte_count != byte_count
        {
            return Err(coordinator_error(
                "The Restore Coordinator receipt differs from its manifest.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) async fn verify_offline(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
        control_export: &[u8],
        archive_path: Option<PathBuf>,
    ) -> UseResult<VerifiedControlRestoreCoordinatorSnapshot> {
        self.validate(registry, expected_binding)?;
        expected_binding.verify_control_export(registry, control_export)?;
        archive::verify_archive(self, archive_path.as_deref()).await?;
        Ok(VerifiedControlRestoreCoordinatorSnapshot {
            entry_count: self.manifest.entries.len(),
        })
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlRestoreCoordinatorSnapshot {
    entry_count: usize,
}

impl VerifiedControlRestoreCoordinatorSnapshot {
    fn entry_count(&self) -> usize {
        self.entry_count
    }
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_restore_coordinator(
        &self,
        destination: PathBuf,
        created_at_ms: u64,
    ) -> UseResult<ControlRestoreCoordinatorSnapshot> {
        let limits = coordinator_contract(self.registry())?;
        let captured = archive::snapshot_live(
            self.state_root(),
            &self.binding().installation,
            destination,
            limits,
        )
        .await?;
        let manifest = ControlRestoreCoordinatorSnapshotManifest::new(
            self.registry(),
            self.binding().clone(),
            created_at_ms,
            captured.payload,
            captured.excluded_active_files,
            captured.excluded_active_inventory_digest,
            captured.entries,
        )?;
        let manifest_bytes = manifest.canonical_bytes(self.registry(), self.binding())?;
        let file_count = manifest.entries.len() as u64;
        let byte_count = manifest.entries.iter().map(|entry| entry.length).sum();
        let receipt = self.receipt(
            ControlPayloadOwnerId::RestoreCoordinator,
            ControlPayloadSnapshotEvidence::new(
                manifest.descriptor_digest.clone(),
                manifest.inventory_digest.clone(),
                manifest_bytes.len() as u64,
                file_count,
                byte_count,
            ),
        )?;
        let snapshot = ControlRestoreCoordinatorSnapshot { manifest, receipt };
        snapshot.validate(self.registry(), self.binding())?;
        let verified = snapshot
            .verify_offline(
                self.registry(),
                self.binding(),
                self.control_export(),
                captured.archive_path,
            )
            .await?;
        if verified.entry_count() != snapshot.manifest.entries.len() {
            return Err(coordinator_error(
                "The new Restore Coordinator snapshot changed before registration.",
            ));
        }
        Ok(snapshot)
    }
}

fn coordinator_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::RestoreCoordinator)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(coordinator_error(
            "The Restore Coordinator owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA {
        return Err(coordinator_error(
            "The Restore Coordinator snapshot schema is unsupported.",
        ));
    }
    Ok(limits)
}

fn restore_inventory_digest(
    installation: &InstallationId,
    entries: &[ControlRestoreCoordinatorEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ControlRestoreCoordinatorEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| coordinator_error(format!("Failed to encode inventory: {error}")))?;
    let mut digest = Sha256::new();
    digest.update(INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn coordinator_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.restore_coordinator_snapshot_invalid",
        message,
    )
}
