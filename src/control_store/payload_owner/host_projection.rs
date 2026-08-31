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
use crate::cognitive_package::{
    HostProjectionSnapshotRecordKind, HOST_PROJECTION_SNAPSHOT_MAX_RECORD_BYTES,
};
use crate::control_store::model::valid_sha256;

mod archive;
mod control;
mod restore;

#[cfg(test)]
pub(in crate::control_store) use restore::{
    ControlHostProjectionRestoreResult, ControlHostProjectionRestoreState,
    StagedControlHostProjectionRestore,
};

pub(in crate::control_store) const CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-host-projection-snapshot.v1";
const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-host-projection-snapshot.v1\0";
const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-host-projection-inventory.v1\0";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlHostProjectionEntryKind {
    Request,
    Cancellation,
}

impl From<HostProjectionSnapshotRecordKind> for ControlHostProjectionEntryKind {
    fn from(value: HostProjectionSnapshotRecordKind) -> Self {
        match value {
            HostProjectionSnapshotRecordKind::Request => Self::Request,
            HostProjectionSnapshotRecordKind::Cancellation => Self::Cancellation,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlHostProjectionEntry {
    pub(in crate::control_store) kind: ControlHostProjectionEntryKind,
    pub(in crate::control_store) path: String,
    pub(in crate::control_store) length: u64,
    pub(in crate::control_store) sha256: String,
}

impl ControlHostProjectionEntry {
    fn validate(&self) -> UseResult<()> {
        let segments = self.path.split('/').collect::<Vec<_>>();
        let [scope_digest, family, file_name] = segments.as_slice() else {
            return Err(host_projection_error(
                "A Host projection entry has a non-canonical path.",
            ));
        };
        let expected_kind = match *family {
            "requests" => ControlHostProjectionEntryKind::Request,
            "cancellations" => ControlHostProjectionEntryKind::Cancellation,
            _ => {
                return Err(host_projection_error(
                    "A Host projection entry is outside its semantic inventory.",
                ))
            }
        };
        let file_digest = file_name.strip_suffix(".json");
        if self.kind != expected_kind
            || !valid_hex_digest(scope_digest)
            || file_digest.is_none_or(|digest| !valid_hex_digest(digest))
            || self.length == 0
            || self.length > HOST_PROJECTION_SNAPSHOT_MAX_RECORD_BYTES
            || !valid_sha256(&self.sha256)
            || !portable_path(&self.path)
        {
            return Err(host_projection_error(
                "A Host projection entry is invalid or exceeds its owner bound.",
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
pub(in crate::control_store) enum ControlHostProjectionState {
    Absent,
    Archive {
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlHostProjectionSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) payload: ControlHostProjectionState,
    pub(in crate::control_store) validated_index_records: u64,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) entries: Vec<ControlHostProjectionEntry>,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlHostProjectionSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        created_at_ms: u64,
        payload: ControlHostProjectionState,
        validated_index_records: u64,
        entries: Vec<ControlHostProjectionEntry>,
    ) -> UseResult<Self> {
        let inventory_digest =
            host_inventory_digest(&binding.installation, validated_index_records, &entries)?;
        let mut manifest = Self {
            schema: CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA.to_owned(),
            binding,
            created_at_ms,
            payload,
            validated_index_records,
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
        let limits = host_projection_contract(registry)?;
        self.binding.validate(registry)?;
        if self.schema != CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA
            || &self.binding != expected_binding
            || self.created_at_ms == 0
            || !valid_sha256(&self.inventory_digest)
            || !valid_sha256(&self.descriptor_digest)
        {
            return Err(host_projection_error(
                "The Host projection manifest is invalid or was rebound.",
            ));
        }
        let mut prior = None;
        let mut portable = BTreeSet::new();
        let mut byte_count = 0_u64;
        for entry in &self.entries {
            entry.validate()?;
            if prior.is_some_and(|prior| prior >= entry.path.as_str())
                || !portable.insert(entry.path.to_ascii_lowercase())
            {
                return Err(host_projection_error(
                    "Host projection entries are not uniquely and portably ordered.",
                ));
            }
            byte_count = byte_count.checked_add(entry.length).ok_or_else(|| {
                host_projection_error("Host projection byte accounting overflowed.")
            })?;
            prior = Some(entry.path.as_str());
        }
        let file_count = u64::try_from(self.entries.len())
            .map_err(|_| host_projection_error("Host projection file accounting overflowed."))?;
        let validated_records = file_count
            .checked_add(self.validated_index_records)
            .ok_or_else(|| host_projection_error("Host record accounting overflowed."))?;
        if validated_records > limits.max_files || byte_count > limits.max_payload_bytes {
            return Err(host_projection_error(
                "The Host projection exceeds its registered file or byte bound.",
            ));
        }
        match &self.payload {
            ControlHostProjectionState::Absent => {
                if file_count != 0 || byte_count != 0 || self.validated_index_records != 0 {
                    return Err(host_projection_error(
                        "An absent Host projection carries nonempty inventory evidence.",
                    ));
                }
            }
            ControlHostProjectionState::Archive {
                archive_bytes,
                archive_sha256,
            } => {
                if file_count == 0 || *archive_bytes != byte_count || !valid_sha256(archive_sha256)
                {
                    return Err(host_projection_error(
                        "Host archive evidence differs from its semantic entries.",
                    ));
                }
            }
        }
        if host_inventory_digest(
            &self.binding.installation,
            self.validated_index_records,
            &self.entries,
        )? != self.inventory_digest
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(host_projection_error(
                "The Host projection manifest digest is inconsistent.",
            ));
        }
        let bytes = canonical_json(self).map_err(|error| {
            host_projection_error(format!(
                "Failed to encode the canonical Host projection manifest: {error}"
            ))
        })?;
        if bytes.is_empty()
            || u64::try_from(bytes.len())
                .ok()
                .is_none_or(|length| length > limits.max_manifest_bytes)
        {
            return Err(host_projection_error(
                "The Host projection manifest exceeds its registered byte bound.",
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
            host_projection_error(format!(
                "Failed to encode the canonical Host projection manifest: {error}"
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
            payload: &'a ControlHostProjectionState,
            validated_index_records: u64,
            inventory_digest: &'a str,
            entries: &'a [ControlHostProjectionEntry],
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            created_at_ms: self.created_at_ms,
            payload: &self.payload,
            validated_index_records: self.validated_index_records,
            inventory_digest: &self.inventory_digest,
            entries: &self.entries,
        })
        .map_err(|error| {
            host_projection_error(format!("Failed to encode Host manifest: {error}"))
        })?;
        let mut digest = Sha256::new();
        digest.update(SNAPSHOT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlHostProjectionSnapshot {
    pub(in crate::control_store) manifest: ControlHostProjectionSnapshotManifest,
    pub(in crate::control_store) receipt: ControlPayloadSnapshotReceipt,
}

impl ControlHostProjectionSnapshot {
    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        self.manifest.validate(registry, expected_binding)?;
        self.receipt.validate(registry, expected_binding)?;
        let manifest_bytes = self.manifest.canonical_bytes(registry, expected_binding)?;
        let file_count = u64::try_from(self.manifest.entries.len())
            .map_err(|_| host_projection_error("Host file accounting overflowed."))?;
        let byte_count = self
            .manifest
            .entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
            .ok_or_else(|| host_projection_error("Host byte accounting overflowed."))?;
        if self.receipt.owner != ControlPayloadOwnerId::HostProtocolProjection
            || self.receipt.owner_manifest_digest != self.manifest.descriptor_digest
            || self.receipt.inventory_digest != self.manifest.inventory_digest
            || self.receipt.manifest_bytes != manifest_bytes.len() as u64
            || self.receipt.file_count != file_count
            || self.receipt.byte_count != byte_count
        {
            return Err(host_projection_error(
                "The Host projection receipt differs from its owner manifest.",
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
    ) -> UseResult<VerifiedControlHostProjectionSnapshot> {
        self.validate(registry, expected_binding)?;
        let authority = expected_binding.verify_control_export(registry, control_export)?;
        let records = archive::verify_archive(self, archive_path.as_deref()).await?;
        control::reconcile(&authority, &records)?;
        Ok(VerifiedControlHostProjectionSnapshot {
            archive_path,
            registry: registry.clone(),
            records,
            snapshot: self.clone(),
        })
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlHostProjectionSnapshot {
    archive_path: Option<PathBuf>,
    registry: ControlPayloadOwnerRegistry,
    records: Vec<crate::cognitive_package::HostProjectionSnapshotRecord>,
    snapshot: ControlHostProjectionSnapshot,
}

impl VerifiedControlHostProjectionSnapshot {
    fn entry_count(&self) -> usize {
        self.records.len()
    }
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_host_projection(
        &self,
        destination: PathBuf,
        created_at_ms: u64,
    ) -> UseResult<ControlHostProjectionSnapshot> {
        let limits = host_projection_contract(self.registry())?;
        let authority = self
            .binding()
            .verify_control_export(self.registry(), self.control_export())?;
        let captured = archive::snapshot_live(
            self.state_root(),
            &self.binding().installation,
            destination,
            limits,
            |records| control::reconcile(&authority, records),
        )
        .await?;
        let manifest = ControlHostProjectionSnapshotManifest::new(
            self.registry(),
            self.binding().clone(),
            created_at_ms,
            captured.payload,
            captured.validated_index_records,
            captured.entries,
        )?;
        let manifest_bytes = manifest.canonical_bytes(self.registry(), self.binding())?;
        let file_count = manifest.entries.len() as u64;
        let byte_count = manifest.entries.iter().map(|entry| entry.length).sum();
        let receipt = self.receipt(
            ControlPayloadOwnerId::HostProtocolProjection,
            ControlPayloadSnapshotEvidence::new(
                manifest.descriptor_digest.clone(),
                manifest.inventory_digest.clone(),
                manifest_bytes.len() as u64,
                file_count,
                byte_count,
            ),
        )?;
        let snapshot = ControlHostProjectionSnapshot { manifest, receipt };
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
            return Err(host_projection_error(
                "The new Host projection changed before registration.",
            ));
        }
        Ok(snapshot)
    }
}

fn host_projection_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::HostProtocolProjection)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(host_projection_error(
            "The Host projection owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA {
        return Err(host_projection_error(
            "The Host projection owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

fn host_inventory_digest(
    installation: &InstallationId,
    validated_index_records: u64,
    entries: &[ControlHostProjectionEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        validated_index_records: u64,
        entries: &'a [ControlHostProjectionEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        validated_index_records,
        entries,
    })
    .map_err(|error| host_projection_error(format!("Failed to encode Host inventory: {error}")))?;
    let mut digest = Sha256::new();
    digest.update(INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn portable_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() <= 1024
        && !path.starts_with('/')
        && !path.ends_with('/')
        && path.split('/').all(|segment| {
            !segment.is_empty()
                && !matches!(segment, "." | "..")
                && segment.len() <= 255
                && !segment.ends_with([' ', '.'])
                && segment.bytes().all(|byte| {
                    byte >= 0x20
                        && byte != 0x7f
                        && !matches!(byte, b'<' | b'>' | b':' | b'"' | b'\\' | b'|' | b'?' | b'*')
                })
        })
}

fn valid_hex_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn host_projection_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.host_projection_snapshot_invalid",
        message,
    )
}
