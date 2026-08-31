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

mod archive;

pub(in crate::control_store) const CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-observation-payload-snapshot.v1";
const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-observation-payload-snapshot.v1\0";
const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-observation-payload-inventory.v1\0";
const MAX_RECORD_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlObservationPayloadEntryKind {
    DiagnosticHistory,
    TerminalResolution,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlObservationPayloadEntry {
    pub(in crate::control_store) kind: ControlObservationPayloadEntryKind,
    pub(in crate::control_store) path: String,
    pub(in crate::control_store) length: u64,
    pub(in crate::control_store) sha256: String,
}

impl ControlObservationPayloadEntry {
    fn validate(&self) -> UseResult<()> {
        let expected_kind = if self.path.starts_with("package-diagnostic-history/scopes/") {
            ControlObservationPayloadEntryKind::DiagnosticHistory
        } else if self.path.starts_with("package-resolutions/install/")
            || self.path.starts_with("package-resolutions/upgrade/")
        {
            ControlObservationPayloadEntryKind::TerminalResolution
        } else {
            return Err(observation_error(
                "An observation payload entry is outside the terminal owner inventory.",
            ));
        };
        if self.kind != expected_kind
            || self.length == 0
            || self.length > MAX_RECORD_BYTES
            || !valid_sha256(&self.sha256)
            || !portable_path(&self.path)
        {
            return Err(observation_error(
                "An observation payload entry is invalid or exceeds its bound.",
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
pub(in crate::control_store) enum ControlObservationPayloadState {
    Absent,
    Archive {
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlObservationPayloadSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) payload: ControlObservationPayloadState,
    pub(in crate::control_store) excluded_active_records: u64,
    pub(in crate::control_store) excluded_active_inventory_digest: String,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) entries: Vec<ControlObservationPayloadEntry>,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlObservationPayloadSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        created_at_ms: u64,
        payload: ControlObservationPayloadState,
        excluded_active_records: u64,
        excluded_active_inventory_digest: String,
        entries: Vec<ControlObservationPayloadEntry>,
    ) -> UseResult<Self> {
        let inventory_digest = observation_inventory_digest(&binding.installation, &entries)?;
        let mut manifest = Self {
            schema: CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA.to_owned(),
            binding,
            created_at_ms,
            payload,
            excluded_active_records,
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
        let limits = observation_contract(registry)?;
        self.binding.validate(registry)?;
        if self.schema != CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA
            || &self.binding != expected_binding
            || self.created_at_ms == 0
            || !valid_sha256(&self.excluded_active_inventory_digest)
            || !valid_sha256(&self.inventory_digest)
            || !valid_sha256(&self.descriptor_digest)
        {
            return Err(observation_error(
                "The observation payload manifest is invalid or was rebound.",
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
                return Err(observation_error(
                    "Observation payload entries are not uniquely and portably ordered.",
                ));
            }
            byte_count = byte_count.checked_add(entry.length).ok_or_else(|| {
                observation_error("Observation payload byte accounting overflowed.")
            })?;
            prior = Some(entry.path.as_str());
        }
        let file_count = u64::try_from(self.entries.len())
            .map_err(|_| observation_error("Observation payload file accounting overflowed."))?;
        let scanned_files = file_count
            .checked_add(self.excluded_active_records)
            .ok_or_else(|| observation_error("Observation record accounting overflowed."))?;
        if scanned_files > limits.max_files || byte_count > limits.max_payload_bytes {
            return Err(observation_error(
                "The observation payload exceeds its registered file or byte bound.",
            ));
        }
        match &self.payload {
            ControlObservationPayloadState::Absent => {
                if file_count != 0 || byte_count != 0 {
                    return Err(observation_error(
                        "An absent observation payload contains terminal entries.",
                    ));
                }
            }
            ControlObservationPayloadState::Archive {
                archive_bytes,
                archive_sha256,
            } => {
                if file_count == 0 || *archive_bytes != byte_count || !valid_sha256(archive_sha256)
                {
                    return Err(observation_error(
                        "Observation archive evidence differs from its terminal entries.",
                    ));
                }
            }
        }
        if observation_inventory_digest(&self.binding.installation, &self.entries)?
            != self.inventory_digest
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(observation_error(
                "The observation payload manifest digest is inconsistent.",
            ));
        }
        let bytes = canonical_json(self).map_err(|error| {
            observation_error(format!(
                "Failed to encode the canonical observation payload manifest: {error}"
            ))
        })?;
        if bytes.is_empty()
            || u64::try_from(bytes.len())
                .ok()
                .is_none_or(|length| length > limits.max_manifest_bytes)
        {
            return Err(observation_error(
                "The observation payload manifest exceeds its registered byte bound.",
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
            observation_error(format!(
                "Failed to encode the canonical observation payload manifest: {error}"
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
            payload: &'a ControlObservationPayloadState,
            excluded_active_records: u64,
            excluded_active_inventory_digest: &'a str,
            inventory_digest: &'a str,
            entries: &'a [ControlObservationPayloadEntry],
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            created_at_ms: self.created_at_ms,
            payload: &self.payload,
            excluded_active_records: self.excluded_active_records,
            excluded_active_inventory_digest: &self.excluded_active_inventory_digest,
            inventory_digest: &self.inventory_digest,
            entries: &self.entries,
        })
        .map_err(|error| observation_error(format!("Failed to encode manifest: {error}")))?;
        let mut digest = Sha256::new();
        digest.update(SNAPSHOT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlObservationPayloadSnapshot {
    pub(in crate::control_store) manifest: ControlObservationPayloadSnapshotManifest,
    pub(in crate::control_store) receipt: ControlPayloadSnapshotReceipt,
}

impl ControlObservationPayloadSnapshot {
    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        self.manifest.validate(registry, expected_binding)?;
        self.receipt.validate(registry, expected_binding)?;
        let manifest_bytes = self.manifest.canonical_bytes(registry, expected_binding)?;
        let file_count = u64::try_from(self.manifest.entries.len())
            .map_err(|_| observation_error("Observation file accounting overflowed."))?;
        let byte_count = self
            .manifest
            .entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
            .ok_or_else(|| observation_error("Observation byte accounting overflowed."))?;
        if self.receipt.owner != ControlPayloadOwnerId::PlanningAndDiagnosticObservations
            || self.receipt.owner_manifest_digest != self.manifest.descriptor_digest
            || self.receipt.inventory_digest != self.manifest.inventory_digest
            || self.receipt.manifest_bytes != manifest_bytes.len() as u64
            || self.receipt.file_count != file_count
            || self.receipt.byte_count != byte_count
        {
            return Err(observation_error(
                "The observation payload receipt differs from its owner manifest.",
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
    ) -> UseResult<VerifiedControlObservationPayloadSnapshot> {
        self.validate(registry, expected_binding)?;
        expected_binding.verify_control_export(registry, control_export)?;
        archive::verify_archive(self, archive_path.as_deref()).await?;
        Ok(VerifiedControlObservationPayloadSnapshot {
            entry_count: self.manifest.entries.len(),
        })
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlObservationPayloadSnapshot {
    entry_count: usize,
}

impl VerifiedControlObservationPayloadSnapshot {
    fn entry_count(&self) -> usize {
        self.entry_count
    }
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_planning_and_diagnostics(
        &self,
        destination: PathBuf,
        created_at_ms: u64,
    ) -> UseResult<ControlObservationPayloadSnapshot> {
        let limits = observation_contract(self.registry())?;
        let captured = archive::snapshot_live(
            self.state_root(),
            &self.binding().installation,
            destination,
            limits,
        )
        .await?;
        let manifest = ControlObservationPayloadSnapshotManifest::new(
            self.registry(),
            self.binding().clone(),
            created_at_ms,
            captured.payload,
            captured.excluded_active_records,
            captured.excluded_active_inventory_digest,
            captured.entries,
        )?;
        let manifest_bytes = manifest.canonical_bytes(self.registry(), self.binding())?;
        let file_count = manifest.entries.len() as u64;
        let byte_count = manifest.entries.iter().map(|entry| entry.length).sum();
        let receipt = self.receipt(
            ControlPayloadOwnerId::PlanningAndDiagnosticObservations,
            ControlPayloadSnapshotEvidence::new(
                manifest.descriptor_digest.clone(),
                manifest.inventory_digest.clone(),
                manifest_bytes.len() as u64,
                file_count,
                byte_count,
            ),
        )?;
        let snapshot = ControlObservationPayloadSnapshot { manifest, receipt };
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
            return Err(observation_error(
                "The new observation snapshot changed before registration.",
            ));
        }
        Ok(snapshot)
    }
}

fn observation_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::PlanningAndDiagnosticObservations)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(observation_error(
            "The observation payload owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA {
        return Err(observation_error(
            "The observation payload owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

fn observation_inventory_digest(
    installation: &InstallationId,
    entries: &[ControlObservationPayloadEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ControlObservationPayloadEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| observation_error(format!("Failed to encode inventory: {error}")))?;
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

fn observation_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.observation_payload_snapshot_invalid",
        message,
    )
}
