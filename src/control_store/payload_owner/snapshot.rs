use a3s_use_core::{InstallationId, UseError, UseResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerRegistration,
    ControlPayloadOwnerRegistry, ControlPayloadSnapshotBinding, MAX_CONTROL_PAYLOAD_OWNER_BYTES,
    MAX_CONTROL_PAYLOAD_OWNER_FILES,
};
use crate::control_store::model::valid_sha256;

pub(in crate::control_store) const CONTROL_PAYLOAD_SNAPSHOT_RECEIPT_SCHEMA: &str =
    "a3s.use.control-payload-snapshot-receipt.v1";
const CONTROL_PAYLOAD_SNAPSHOT_SET_SCHEMA: &str = "a3s.use.control-payload-snapshot-set.v1";
const MAX_CONTROL_PAYLOAD_SNAPSHOT_SET_BYTES: usize = 1024 * 1024;
const MAX_CONTROL_PAYLOAD_MANIFEST_TOTAL_BYTES: u64 = 64 * 1024 * 1024;

/// Canonical evidence returned by one external payload owner's snapshot.
///
/// Paths are intentionally absent. Each owner remains responsible for its own
/// snapshot format while the Control backup coordinator records only bounded,
/// generation-bound evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) struct ControlPayloadSnapshotEvidence {
    owner_manifest_digest: String,
    inventory_digest: String,
    manifest_bytes: u64,
    file_count: u64,
    byte_count: u64,
}

impl ControlPayloadSnapshotEvidence {
    pub(in crate::control_store) fn new(
        owner_manifest_digest: impl Into<String>,
        inventory_digest: impl Into<String>,
        manifest_bytes: u64,
        file_count: u64,
        byte_count: u64,
    ) -> Self {
        Self {
            owner_manifest_digest: owner_manifest_digest.into(),
            inventory_digest: inventory_digest.into(),
            manifest_bytes,
            file_count,
            byte_count,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPayloadSnapshotReceipt {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) owner: ControlPayloadOwnerId,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) control_generation: u64,
    pub(in crate::control_store) owner_snapshot_schema: String,
    pub(in crate::control_store) owner_manifest_digest: String,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) manifest_bytes: u64,
    pub(in crate::control_store) file_count: u64,
    pub(in crate::control_store) byte_count: u64,
}

impl ControlPayloadSnapshotReceipt {
    pub(in crate::control_store) fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: &ControlPayloadSnapshotBinding,
        owner: ControlPayloadOwnerId,
        evidence: ControlPayloadSnapshotEvidence,
    ) -> UseResult<Self> {
        registry.validate()?;
        binding.validate(registry)?;
        let owner_snapshot_schema = registry
            .registration(owner)
            .and_then(ControlPayloadOwnerRegistration::snapshot_contract)
            .map(|(schema, _)| schema.to_string())
            .ok_or_else(|| {
                snapshot_error("The Control payload owner does not produce backup snapshots.")
            })?;
        let receipt = Self {
            schema: CONTROL_PAYLOAD_SNAPSHOT_RECEIPT_SCHEMA.to_string(),
            owner,
            installation: binding.installation.clone(),
            control_generation: binding.control_generation,
            owner_snapshot_schema,
            owner_manifest_digest: evidence.owner_manifest_digest,
            inventory_digest: evidence.inventory_digest,
            manifest_bytes: evidence.manifest_bytes,
            file_count: evidence.file_count,
            byte_count: evidence.byte_count,
        };
        receipt.validate(registry, binding)?;
        Ok(receipt)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        registry.validate()?;
        binding.validate(registry)?;
        let Some((expected_schema, limits)) = registry
            .registration(self.owner)
            .and_then(ControlPayloadOwnerRegistration::snapshot_contract)
        else {
            return Err(snapshot_error(
                "An excluded Control payload owner produced a snapshot receipt.",
            ));
        };
        if self.schema != CONTROL_PAYLOAD_SNAPSHOT_RECEIPT_SCHEMA
            || self.installation.validate().is_err()
            || self.installation != binding.installation
            || self.control_generation != binding.control_generation
            || self.owner_snapshot_schema != expected_schema
            || !valid_sha256(&self.owner_manifest_digest)
            || !valid_sha256(&self.inventory_digest)
            || self.manifest_bytes == 0
            || self.manifest_bytes > limits.max_manifest_bytes
            || self.file_count > limits.max_files
            || self.byte_count > limits.max_payload_bytes
            || (self.file_count == 0 && self.byte_count != 0)
        {
            return Err(snapshot_error(
                "A Control payload snapshot receipt is invalid or exceeds its registered bounds.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlPayloadSnapshotSet {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) manifest_bytes: u64,
    pub(in crate::control_store) file_count: u64,
    pub(in crate::control_store) byte_count: u64,
    pub(in crate::control_store) receipts: Vec<ControlPayloadSnapshotReceipt>,
}

impl ControlPayloadSnapshotSet {
    pub(in crate::control_store) fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        mut receipts: Vec<ControlPayloadSnapshotReceipt>,
    ) -> UseResult<Self> {
        receipts.sort_by_key(|receipt| receipt.owner);
        let (manifest_bytes, file_count, byte_count) = totals(&receipts)?;
        let snapshot = Self {
            schema: CONTROL_PAYLOAD_SNAPSHOT_SET_SCHEMA.to_string(),
            binding,
            manifest_bytes,
            file_count,
            byte_count,
            receipts,
        };
        snapshot.validate(registry)?;
        Ok(snapshot)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        registry.validate()?;
        self.binding.validate(registry)?;
        let owners = self
            .receipts
            .iter()
            .map(|receipt| receipt.owner)
            .collect::<Vec<_>>();
        if self.schema != CONTROL_PAYLOAD_SNAPSHOT_SET_SCHEMA
            || owners.as_slice() != ControlPayloadOwnerId::SNAPSHOTTED
        {
            return Err(snapshot_error(
                "The Control payload snapshot set is incomplete or bound to another registry.",
            ));
        }
        for receipt in &self.receipts {
            receipt.validate(registry, &self.binding)?;
        }
        let (manifest_bytes, file_count, byte_count) = totals(&self.receipts)?;
        if self.manifest_bytes != manifest_bytes
            || self.file_count != file_count
            || self.byte_count != byte_count
        {
            return Err(snapshot_error(
                "The Control payload snapshot accounting does not match its receipts.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn descriptor_digest(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<String> {
        self.validate(registry)?;
        let bytes = canonical_json(self).map_err(|error| {
            snapshot_error(format!(
                "Failed to encode the canonical Control payload snapshot set: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_CONTROL_PAYLOAD_SNAPSHOT_SET_BYTES {
            return Err(snapshot_error(
                "The Control payload snapshot set exceeds its canonical byte bound.",
            ));
        }
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }
}

fn totals(receipts: &[ControlPayloadSnapshotReceipt]) -> UseResult<(u64, u64, u64)> {
    let mut manifest_bytes = 0_u64;
    let mut file_count = 0_u64;
    let mut byte_count = 0_u64;
    for receipt in receipts {
        manifest_bytes = manifest_bytes
            .checked_add(receipt.manifest_bytes)
            .ok_or_else(|| snapshot_error("Control payload manifest accounting overflowed."))?;
        file_count = file_count
            .checked_add(receipt.file_count)
            .ok_or_else(|| snapshot_error("Control payload file accounting overflowed."))?;
        byte_count = byte_count
            .checked_add(receipt.byte_count)
            .ok_or_else(|| snapshot_error("Control payload byte accounting overflowed."))?;
    }
    if manifest_bytes > MAX_CONTROL_PAYLOAD_MANIFEST_TOTAL_BYTES
        || file_count > MAX_CONTROL_PAYLOAD_OWNER_FILES
        || byte_count > MAX_CONTROL_PAYLOAD_OWNER_BYTES
    {
        return Err(snapshot_error(
            "The combined Control payload snapshot exceeds its global bounds.",
        ));
    }
    Ok((manifest_bytes, file_count, byte_count))
}

fn snapshot_error(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.payload_snapshot_invalid", message)
}
