use a3s_use_core::{UseError, UseResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::host_projection::{ControlHostProjectionSnapshot, ControlHostProjectionState};
use super::knowledge::{ControlKnowledgePayloadSnapshot, ControlKnowledgePayloadState};
use super::observations::{ControlObservationPayloadSnapshot, ControlObservationPayloadState};
use super::restore_coordinator::{
    ControlRestoreCoordinatorSnapshot, ControlRestoreCoordinatorState,
};
use super::{canonical_json, ControlPayloadOwnerRegistry, ControlPayloadSnapshotSet};
use crate::control_store::export::MAX_CONTROL_STORE_EXPORT_BYTES;
use crate::control_store::model::valid_sha256;

mod archive;
mod coordinator;

#[cfg(test)]
pub(in crate::control_store) use coordinator::VerifiedControlInstallationSnapshot;

const COMPLETE_SNAPSHOT_SCHEMA: &str = "a3s.use.control-installation-snapshot.v1";
const COMPLETE_SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-installation-snapshot.v1\0";
const MAX_COMPLETE_SNAPSHOT_MANIFEST_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlInstallationSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) control_export_bytes: u64,
    pub(in crate::control_store) snapshot_set: ControlPayloadSnapshotSet,
    pub(in crate::control_store) host_projection: ControlHostProjectionSnapshot,
    pub(in crate::control_store) knowledge: ControlKnowledgePayloadSnapshot,
    pub(in crate::control_store) observations: ControlObservationPayloadSnapshot,
    pub(in crate::control_store) restore_coordinator: ControlRestoreCoordinatorSnapshot,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlInstallationSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        created_at_ms: u64,
        control_export_bytes: u64,
        snapshot_set: ControlPayloadSnapshotSet,
        owners: CapturedOwnerSnapshots,
    ) -> UseResult<Self> {
        let mut manifest = Self {
            schema: COMPLETE_SNAPSHOT_SCHEMA.to_owned(),
            created_at_ms,
            control_export_bytes,
            snapshot_set,
            host_projection: owners.host_projection,
            knowledge: owners.knowledge,
            observations: owners.observations,
            restore_coordinator: owners.restore_coordinator,
            descriptor_digest: String::new(),
        };
        manifest.descriptor_digest = manifest.expected_descriptor_digest()?;
        manifest.validate(registry)?;
        Ok(manifest)
    }

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        registry
            .validate()
            .map_err(|error| nested_snapshot_invalid("owner registry", error))?;
        self.snapshot_set
            .validate(registry)
            .map_err(|error| nested_snapshot_invalid("snapshot set", error))?;
        let binding = &self.snapshot_set.binding;
        self.host_projection
            .validate(registry, binding)
            .map_err(|error| nested_snapshot_invalid("Host projection", error))?;
        self.knowledge
            .validate(registry, binding)
            .map_err(|error| nested_snapshot_invalid("Knowledge payload", error))?;
        self.observations
            .validate(registry, binding)
            .map_err(|error| nested_snapshot_invalid("observation payload", error))?;
        self.restore_coordinator
            .validate(registry, binding)
            .map_err(|error| nested_snapshot_invalid("Restore Coordinator", error))?;

        let expected_receipts = vec![
            self.host_projection.receipt.clone(),
            self.knowledge.receipt.clone(),
            self.observations.receipt.clone(),
            self.restore_coordinator.receipt.clone(),
        ];
        let timestamps = [
            self.host_projection.manifest.created_at_ms,
            self.knowledge.manifest.created_at_ms,
            self.observations.manifest.created_at_ms,
            self.restore_coordinator.manifest.created_at_ms,
        ];
        if self.schema != COMPLETE_SNAPSHOT_SCHEMA
            || self.created_at_ms == 0
            || timestamps.iter().any(|value| *value != self.created_at_ms)
            || self.control_export_bytes == 0
            || self.control_export_bytes > MAX_CONTROL_STORE_EXPORT_BYTES as u64
            || self.snapshot_set.receipts != expected_receipts
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(snapshot_invalid(
                "The complete Control snapshot is incomplete, noncanonical, or was rebound.",
            ));
        }
        self.archive_entries()?;
        Ok(())
    }

    fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate_without_digest()?;
        let bytes = canonical_json(self).map_err(|error| {
            snapshot_invalid(format!(
                "Failed to encode the complete Control snapshot manifest: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_COMPLETE_SNAPSHOT_MANIFEST_BYTES {
            return Err(snapshot_invalid(
                "The complete Control snapshot manifest exceeds its byte bound.",
            ));
        }
        Ok(bytes)
    }

    fn expected_descriptor_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            created_at_ms: u64,
            control_export_bytes: u64,
            snapshot_set: &'a ControlPayloadSnapshotSet,
            host_projection: &'a ControlHostProjectionSnapshot,
            knowledge: &'a ControlKnowledgePayloadSnapshot,
            observations: &'a ControlObservationPayloadSnapshot,
            restore_coordinator: &'a ControlRestoreCoordinatorSnapshot,
        }

        self.validate_without_digest()?;
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            created_at_ms: self.created_at_ms,
            control_export_bytes: self.control_export_bytes,
            snapshot_set: &self.snapshot_set,
            host_projection: &self.host_projection,
            knowledge: &self.knowledge,
            observations: &self.observations,
            restore_coordinator: &self.restore_coordinator,
        })
        .map_err(|error| {
            snapshot_invalid(format!(
                "Failed to encode the complete Control snapshot descriptor: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_COMPLETE_SNAPSHOT_MANIFEST_BYTES {
            return Err(snapshot_invalid(
                "The complete Control snapshot descriptor exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(COMPLETE_SNAPSHOT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }

    fn validate_without_digest(&self) -> UseResult<()> {
        if self.schema != COMPLETE_SNAPSHOT_SCHEMA
            || self.created_at_ms == 0
            || self.control_export_bytes == 0
            || self.control_export_bytes > MAX_CONTROL_STORE_EXPORT_BYTES as u64
        {
            return Err(snapshot_invalid(
                "The complete Control snapshot identity or Control export size is invalid.",
            ));
        }
        Ok(())
    }

    fn archive_entries(&self) -> UseResult<Vec<ArchiveEntry>> {
        let mut entries = vec![ArchiveEntry {
            kind: ArchiveEntryKind::ControlExport,
            length: self.control_export_bytes,
            sha256: self.snapshot_set.binding.control_export_digest.clone(),
        }];
        append_optional_entry(
            &mut entries,
            ArchiveEntryKind::HostProjection,
            match &self.host_projection.manifest.payload {
                ControlHostProjectionState::Absent => None,
                ControlHostProjectionState::Archive {
                    archive_bytes,
                    archive_sha256,
                } => Some((*archive_bytes, archive_sha256)),
            },
        )?;
        append_optional_entry(
            &mut entries,
            ArchiveEntryKind::Knowledge,
            match &self.knowledge.manifest.payload {
                ControlKnowledgePayloadState::Absent => None,
                ControlKnowledgePayloadState::Archive {
                    archive_bytes,
                    archive_sha256,
                    ..
                } => Some((*archive_bytes, archive_sha256)),
            },
        )?;
        append_optional_entry(
            &mut entries,
            ArchiveEntryKind::Observations,
            match &self.observations.manifest.payload {
                ControlObservationPayloadState::Absent => None,
                ControlObservationPayloadState::Archive {
                    archive_bytes,
                    archive_sha256,
                } => Some((*archive_bytes, archive_sha256)),
            },
        )?;
        append_optional_entry(
            &mut entries,
            ArchiveEntryKind::RestoreCoordinator,
            match &self.restore_coordinator.manifest.payload {
                ControlRestoreCoordinatorState::Absent => None,
                ControlRestoreCoordinatorState::Archive {
                    archive_bytes,
                    archive_sha256,
                } => Some((*archive_bytes, archive_sha256)),
            },
        )?;
        let owner_bytes = entries
            .iter()
            .skip(1)
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
            .ok_or_else(|| snapshot_invalid("Complete snapshot byte accounting overflowed."))?;
        if owner_bytes != self.snapshot_set.byte_count {
            return Err(snapshot_invalid(
                "The complete snapshot payload bytes differ from owner accounting.",
            ));
        }
        Ok(entries)
    }
}

struct CapturedOwnerSnapshots {
    host_projection: ControlHostProjectionSnapshot,
    knowledge: ControlKnowledgePayloadSnapshot,
    observations: ControlObservationPayloadSnapshot,
    restore_coordinator: ControlRestoreCoordinatorSnapshot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveEntryKind {
    ControlExport,
    HostProjection,
    Knowledge,
    Observations,
    RestoreCoordinator,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ArchiveEntry {
    kind: ArchiveEntryKind,
    length: u64,
    sha256: String,
}

fn append_optional_entry(
    entries: &mut Vec<ArchiveEntry>,
    kind: ArchiveEntryKind,
    evidence: Option<(u64, &String)>,
) -> UseResult<()> {
    if let Some((length, sha256)) = evidence {
        if length == 0 || !valid_sha256(sha256) {
            return Err(snapshot_invalid(
                "A complete snapshot payload has invalid archive evidence.",
            ));
        }
        entries.push(ArchiveEntry {
            kind,
            length,
            sha256: sha256.clone(),
        });
    }
    Ok(())
}

fn snapshot_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.complete_snapshot_invalid", message)
}

fn nested_snapshot_invalid(owner: &str, error: UseError) -> UseError {
    snapshot_invalid(format!(
        "The complete snapshot {owner} evidence is invalid: {}",
        error.message
    ))
}

fn snapshot_path_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.complete_snapshot_path_invalid", message)
}

fn snapshot_exists() -> UseError {
    UseError::new(
        "use.control_store.complete_snapshot_exists",
        "The complete snapshot destination already exists and will not be overwritten.",
    )
}

fn snapshot_io(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.complete_snapshot_io",
        format!("Failed to {0}", message.into()),
    )
}
