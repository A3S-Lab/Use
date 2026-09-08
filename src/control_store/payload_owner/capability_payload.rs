//! Snapshot and clean-target restore for the installation-scoped Capability
//! Gateway catalog and descriptor-snapshot payload family.
//!
//! Capability payload records are immutable projections. They are not a second
//! source of desired state: the Control export still decides which capability
//! generation is live. The owner only preserves the exact catalog and
//! descriptor-snapshot bytes that a Capability Gateway reader needs after
//! restart or restore.

use std::collections::BTreeSet;
use std::io;
use std::path::{Component, Path, PathBuf};

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::StateMaintenanceGuard;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{
    canonical_json, ControlPayloadOwnerId, ControlPayloadOwnerLimits, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding, ControlPayloadSnapshotEvidence, ControlPayloadSnapshotReceipt,
    ControlPayloadSnapshotSession,
};
use crate::capability_catalog_store::{
    CapabilityGatewayCatalogStore, CapabilityGatewayCatalogStoredRecord,
    MAX_CAPABILITY_GATEWAY_CATALOG_BYTES,
};
use crate::control_store::effect_owner::capability_plane::{
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilityDescriptorSnapshotStoredRecord,
    MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES,
};
use crate::control_store::model::valid_sha256;

pub(in crate::control_store) const CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-capability-payload-snapshot.v1";
const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-capability-payload-snapshot.v1\0";
const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-capability-payload-inventory.v1\0";
const ARCHIVE_FILE: &str = "capability-payload.archive";
const ARCHIVE_PARTIAL_FILE: &str = "capability-payload.archive.partial";
const ACTIVATION_FILE: &str = "capability-payload.activating.json";
const ACTIVATION_PARTIAL_FILE: &str = "capability-payload.activating.json.partial";
const CANDIDATE_DIRECTORY: &str = "capability-gateway";
const CATALOGS_DIRECTORY: &str = "catalogs";
const DESCRIPTOR_SNAPSHOTS_DIRECTORY: &str = "descriptor-snapshots";
const ACTIVATION_SCHEMA: &str = "a3s.use.control-capability-payload-activation.v1";
const MAX_ACTIVATION_BYTES: u64 = 16 * 1024;
const MAX_ARCHIVE_RECORD_BYTES: u64 = if MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
    > MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64
{
    MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
} else {
    MAX_CONTROL_CAPABILITY_DESCRIPTOR_SNAPSHOT_BYTES as u64
};

fn candidate_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(CANDIDATE_DIRECTORY)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlCapabilityPayloadState {
    Absent,
    Archive {
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(in crate::control_store) enum ControlCapabilityPayloadEntryKind {
    Catalog,
    DescriptorSnapshot,
}

impl ControlCapabilityPayloadEntryKind {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Catalog => "catalog",
            Self::DescriptorSnapshot => "descriptor-snapshot",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadEntry {
    pub(in crate::control_store) kind: ControlCapabilityPayloadEntryKind,
    pub(in crate::control_store) digest: String,
    pub(in crate::control_store) length: u64,
    pub(in crate::control_store) sha256: String,
}

impl ControlCapabilityPayloadEntry {
    fn validate(&self, _installation: &InstallationId) -> UseResult<()> {
        if !valid_sha256(&self.digest)
            || self.length == 0
            || self.length > MAX_ARCHIVE_RECORD_BYTES
            || !valid_sha256(&self.sha256)
            || self.digest != self.sha256
        {
            return Err(capability_payload_error(
                "A Capability payload entry is invalid or exceeds its bound.",
            ));
        }
        Ok(())
    }

    fn sort_key(&self) -> (u8, &str, &str) {
        let rank = match self.kind {
            ControlCapabilityPayloadEntryKind::Catalog => 0,
            ControlCapabilityPayloadEntryKind::DescriptorSnapshot => 1,
        };
        (rank, self.kind.as_str(), self.digest.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) payload: ControlCapabilityPayloadState,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) entries: Vec<ControlCapabilityPayloadEntry>,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlCapabilityPayloadSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        created_at_ms: u64,
        payload: ControlCapabilityPayloadState,
        entries: Vec<ControlCapabilityPayloadEntry>,
    ) -> UseResult<Self> {
        let inventory_digest = inventory_digest(&binding.installation, &entries)?;
        let mut manifest = Self {
            schema: CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA.to_owned(),
            binding,
            created_at_ms,
            payload,
            inventory_digest,
            entries,
            descriptor_digest: String::new(),
        };
        manifest.descriptor_digest = manifest.expected_descriptor_digest()?;
        manifest.validate(registry, &manifest.binding.clone())?;
        Ok(manifest)
    }

    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        expected_binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<()> {
        let limits = capability_payload_contract(registry)?;
        self.binding.validate(registry)?;
        if self.schema != CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA
            || &self.binding != expected_binding
            || self.created_at_ms == 0
            || !valid_sha256(&self.inventory_digest)
            || !valid_sha256(&self.descriptor_digest)
        {
            return Err(capability_payload_error(
                "The Capability payload manifest is invalid or was rebound.",
            ));
        }

        let mut previous: Option<(u8, &str, &str)> = None;
        let mut identities = BTreeSet::new();
        let mut byte_count = 0_u64;
        for entry in &self.entries {
            entry.validate(&self.binding.installation)?;
            let key = entry.sort_key();
            if previous.is_some_and(|value| value >= key)
                || !identities.insert((entry.kind, entry.digest.as_str()))
            {
                return Err(capability_payload_error(
                    "Capability payload entries are not sorted and unique.",
                ));
            }
            byte_count = byte_count.checked_add(entry.length).ok_or_else(|| {
                capability_payload_error("Capability payload byte accounting overflowed.")
            })?;
            previous = Some(key);
        }
        let file_count = u64::try_from(self.entries.len()).map_err(|_| {
            capability_payload_error("Capability payload file accounting overflowed.")
        })?;
        if file_count > limits.max_files || byte_count > limits.max_payload_bytes {
            return Err(capability_payload_error(
                "The Capability payload exceeds its registered bounds.",
            ));
        }
        match &self.payload {
            ControlCapabilityPayloadState::Absent => {
                if !self.entries.is_empty() || byte_count != 0 {
                    return Err(capability_payload_error(
                        "An absent Capability payload contains records.",
                    ));
                }
            }
            ControlCapabilityPayloadState::Archive {
                archive_bytes,
                archive_sha256,
            } => {
                if self.entries.is_empty()
                    || *archive_bytes != byte_count
                    || !valid_sha256(archive_sha256)
                {
                    return Err(capability_payload_error(
                        "Capability payload archive evidence differs from its entries.",
                    ));
                }
            }
        }
        if inventory_digest(&self.binding.installation, &self.entries)? != self.inventory_digest
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(capability_payload_error(
                "The Capability payload manifest digest is inconsistent.",
            ));
        }
        let bytes = canonical_json(self).map_err(|error| {
            capability_payload_error(format!(
                "Failed to encode the Capability payload manifest: {error}"
            ))
        })?;
        if bytes.is_empty()
            || u64::try_from(bytes.len())
                .ok()
                .is_none_or(|length| length > limits.max_manifest_bytes)
        {
            return Err(capability_payload_error(
                "The Capability payload manifest exceeds its registered bound.",
            ));
        }
        Ok(())
    }

    fn canonical_bytes(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        binding: &ControlPayloadSnapshotBinding,
    ) -> UseResult<Vec<u8>> {
        self.validate(registry, binding)?;
        canonical_json(self).map_err(|error| {
            capability_payload_error(format!(
                "Failed to encode the Capability payload manifest: {error}"
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
            payload: &'a ControlCapabilityPayloadState,
            inventory_digest: &'a str,
            entries: &'a [ControlCapabilityPayloadEntry],
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            created_at_ms: self.created_at_ms,
            payload: &self.payload,
            inventory_digest: &self.inventory_digest,
            entries: &self.entries,
        })
        .map_err(|error| {
            capability_payload_error(format!(
                "Failed to encode the Capability payload descriptor: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(SNAPSHOT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadSnapshot {
    pub(in crate::control_store) manifest: ControlCapabilityPayloadSnapshotManifest,
    pub(in crate::control_store) receipt: ControlPayloadSnapshotReceipt,
}

impl ControlCapabilityPayloadSnapshot {
    pub(in crate::control_store) fn validate(
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
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
            .ok_or_else(|| capability_payload_error("Capability payload accounting overflowed."))?;
        if self.receipt.owner != ControlPayloadOwnerId::CapabilityPayload
            || self.receipt.owner_manifest_digest != self.manifest.descriptor_digest
            || self.receipt.inventory_digest != self.manifest.inventory_digest
            || self.receipt.manifest_bytes != manifest_bytes.len() as u64
            || self.receipt.file_count != file_count
            || self.receipt.byte_count != byte_count
        {
            return Err(capability_payload_error(
                "The Capability payload receipt differs from its manifest.",
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
    ) -> UseResult<VerifiedControlCapabilityPayloadSnapshot> {
        self.validate(registry, expected_binding)?;
        expected_binding.verify_control_export(registry, control_export)?;
        verify_archive(self, archive_path.as_deref()).await?;
        Ok(VerifiedControlCapabilityPayloadSnapshot {
            archive_path,
            registry: registry.clone(),
            snapshot: self.clone(),
        })
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlCapabilityPayloadSnapshot {
    archive_path: Option<PathBuf>,
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlCapabilityPayloadSnapshot,
}

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlCapabilityPayloadRestore {
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlCapabilityPayloadSnapshot,
    state_root: PathBuf,
    staging_directory: PathBuf,
    candidate: Option<PathBuf>,
    activation_bytes: Vec<u8>,
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_capability_payload(
        &self,
        destination: PathBuf,
        created_at_ms: u64,
    ) -> UseResult<ControlCapabilityPayloadSnapshot> {
        let limits = capability_payload_contract(self.registry())?;
        let catalogs = CapabilityGatewayCatalogStore::new(
            self.state_root().to_path_buf(),
            self.binding().installation.clone(),
        )?;
        let descriptors = ControlCapabilityDescriptorSnapshotStore::new(
            self.state_root().to_path_buf(),
            self.binding().installation.clone(),
        )?;
        let captured = snapshot_live(
            &catalogs,
            &descriptors,
            self.maintenance(),
            destination,
            limits,
        )
        .await?;
        let manifest = ControlCapabilityPayloadSnapshotManifest::new(
            self.registry(),
            self.binding().clone(),
            created_at_ms,
            captured.payload,
            captured.entries,
        )?;
        let manifest_bytes = manifest.canonical_bytes(self.registry(), self.binding())?;
        let file_count = manifest.entries.len() as u64;
        let byte_count = manifest
            .entries
            .iter()
            .try_fold(0_u64, |total, entry| total.checked_add(entry.length))
            .ok_or_else(|| capability_payload_error("Capability payload accounting overflowed."))?;
        let receipt = self.receipt(
            ControlPayloadOwnerId::CapabilityPayload,
            ControlPayloadSnapshotEvidence::new(
                manifest.descriptor_digest.clone(),
                manifest.inventory_digest.clone(),
                manifest_bytes.len() as u64,
                file_count,
                byte_count,
            ),
        )?;
        let snapshot = ControlCapabilityPayloadSnapshot { manifest, receipt };
        snapshot.validate(self.registry(), self.binding())?;
        snapshot
            .verify_offline(
                self.registry(),
                self.binding(),
                self.control_export(),
                captured.archive_path,
            )
            .await?;
        Ok(snapshot)
    }
}

impl VerifiedControlCapabilityPayloadSnapshot {
    pub(in crate::control_store) async fn stage_clean_restore_under_exclusive(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlCapabilityPayloadRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        validate_staging_location(&state_root, &staging_directory)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Capability payload restore staging requires the exact target's exclusive maintenance guard.",
            ));
        }
        ensure_owned_directory(&state_root, &staging_directory).await?;
        validate_staging_entries(&staging_directory).await?;
        let activation_bytes = activation_bytes(&self.snapshot)?;
        let activation_started =
            recover_activation_marker(&staging_directory, &activation_bytes).await?;
        let candidate = match (
            &self.snapshot.manifest.payload,
            self.archive_path.as_deref(),
        ) {
            (ControlCapabilityPayloadState::Absent, None) => {
                require_empty_staging(&staging_directory).await?;
                None
            }
            (ControlCapabilityPayloadState::Archive { .. }, Some(source)) => {
                let archive =
                    stage_archive_file(source, &staging_directory, &self.snapshot).await?;
                let (catalogs, descriptors) =
                    read_archive_records(&archive, &self.snapshot).await?;
                let candidate = candidate_path(&staging_directory);
                // Once the owner root has been published, the candidate is moved out
                // of staging.  Reopening that crash window must not recreate an
                // empty candidate beside the already-live root; the activation
                // marker is the durable evidence that permits replay.
                if !activation_started || owned_directory(&candidate).await? {
                    ensure_owned_directory(&staging_directory, &candidate).await?;
                    materialize_candidate(
                        &candidate,
                        &self.snapshot.manifest.binding.installation,
                        &catalogs,
                        &descriptors,
                    )
                    .await?;
                }
                Some(candidate)
            }
            _ => {
                return Err(restore_invalid(
                    "The verified Capability payload snapshot omitted or added archive bytes.",
                ))
            }
        };
        validate_staging_entries(&staging_directory).await?;
        if let Some(candidate) = &candidate {
            if owned_directory(candidate).await? {
                validate_candidate(candidate, &self.snapshot).await?;
            }
        }
        Ok(StagedControlCapabilityPayloadRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            state_root,
            staging_directory,
            candidate,
            activation_bytes,
        })
    }
}

impl StagedControlCapabilityPayloadRestore {
    pub(in crate::control_store) fn candidate_path(&self) -> Option<&Path> {
        self.candidate.as_deref()
    }

    pub(in crate::control_store) async fn preflight_clean(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<()> {
        self.ensure_guard(maintenance)?;
        validate_staging_entries(&self.staging_directory).await?;
        let live = inspect_live_root(&self.state_root).await?;
        if live.is_some() {
            return Err(restore_target_not_empty());
        }
        if recover_activation_marker(&self.staging_directory, &self.activation_bytes).await? {
            return Err(restore_invalid(
                "Capability payload activation evidence exists before complete restore intent.",
            ));
        }
        match (&self.snapshot.manifest.payload, &self.candidate) {
            (ControlCapabilityPayloadState::Absent, None) => {
                require_empty_staging(&self.staging_directory).await
            }
            (ControlCapabilityPayloadState::Archive { .. }, Some(candidate)) => {
                let archive = staged_archive(&self.staging_directory).await?;
                verify_archive(&self.snapshot, Some(&archive)).await?;
                if !owned_directory(candidate).await? {
                    return Err(restore_invalid(
                        "The Capability payload restore candidate disappeared before activation.",
                    ));
                }
                validate_candidate(candidate, &self.snapshot).await
            }
            _ => Err(restore_invalid(
                "The staged Capability payload differs from its snapshot state.",
            )),
        }
    }

    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlCapabilityPayloadRestoreResult> {
        self.ensure_guard(maintenance)?;
        validate_staging_entries(&self.staging_directory).await?;
        match (&self.snapshot.manifest.payload, &self.candidate) {
            (ControlCapabilityPayloadState::Absent, None) => {
                require_empty_staging(&self.staging_directory).await?;
                if inspect_live_root(&self.state_root).await?.is_some() {
                    return Err(restore_target_not_empty());
                }
            }
            (ControlCapabilityPayloadState::Archive { .. }, Some(candidate)) => {
                let archive = staged_archive(&self.staging_directory).await?;
                verify_archive(&self.snapshot, Some(&archive)).await?;
                let activation_started =
                    recover_activation_marker(&self.staging_directory, &self.activation_bytes)
                        .await?;
                match (
                    owned_directory(candidate).await?,
                    inspect_live_root(&self.state_root).await?,
                ) {
                    (true, None) => {
                        validate_candidate(candidate, &self.snapshot).await?;
                        if !activation_started {
                            create_activation_marker(
                                &self.staging_directory,
                                &self.activation_bytes,
                            )
                            .await?;
                            validate_candidate(candidate, &self.snapshot).await?;
                            if inspect_live_root(&self.state_root).await?.is_some() {
                                return Err(restore_target_not_empty());
                            }
                        }
                        publish_directory(
                            candidate.clone(),
                            self.state_root.join(CANDIDATE_DIRECTORY),
                        )
                        .await?;
                    }
                    (false, Some(live)) if activation_started => {
                        validate_live_root(&live, &self.snapshot).await?;
                    }
                    (true, Some(_)) | (false, Some(_)) => return Err(restore_target_not_empty()),
                    (false, None) => return Err(restore_invalid(
                        "The Capability payload restore candidate disappeared before activation.",
                    )),
                }
                let live = inspect_live_root(&self.state_root).await?.ok_or_else(|| {
                    restore_invalid("The Capability payload restore did not publish its live root.")
                })?;
                validate_live_root(&live, &self.snapshot).await?;
                if !recover_activation_marker(&self.staging_directory, &self.activation_bytes)
                    .await?
                {
                    return Err(restore_invalid(
                        "The Capability payload activation marker disappeared before restore completion.",
                    ));
                }
            }
            _ => {
                return Err(restore_invalid(
                    "The staged Capability payload differs from its snapshot state.",
                ))
            }
        }
        ControlCapabilityPayloadRestoreResult::new(&self.registry, &self.snapshot)
    }

    fn ensure_guard(&self, maintenance: &StateMaintenanceGuard) -> UseResult<()> {
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Capability payload restore requires the exact target's exclusive maintenance guard.",
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
pub(in crate::control_store) enum ControlCapabilityPayloadRestoreState {
    Absent,
    Archive {
        records: u64,
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlCapabilityPayloadRestoreResult {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    inventory_digest: String,
    pub(in crate::control_store) payload: ControlCapabilityPayloadRestoreState,
    descriptor_digest: String,
}

impl ControlCapabilityPayloadRestoreResult {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlCapabilityPayloadSnapshot,
    ) -> UseResult<Self> {
        let payload = match &snapshot.manifest.payload {
            ControlCapabilityPayloadState::Absent => ControlCapabilityPayloadRestoreState::Absent,
            ControlCapabilityPayloadState::Archive {
                archive_bytes,
                archive_sha256,
            } => ControlCapabilityPayloadRestoreState::Archive {
                records: snapshot.manifest.entries.len() as u64,
                archive_bytes: *archive_bytes,
                archive_sha256: archive_sha256.clone(),
            },
        };
        let mut result = Self {
            schema: "a3s.use.control-capability-payload-restore-result.v1".to_owned(),
            binding: snapshot.manifest.binding.clone(),
            owner_manifest_digest: snapshot.manifest.descriptor_digest.clone(),
            inventory_digest: snapshot.manifest.inventory_digest.clone(),
            payload,
            descriptor_digest: String::new(),
        };
        result.descriptor_digest = result.expected_digest()?;
        result.validate_for_snapshot(registry, snapshot)?;
        Ok(result)
    }

    fn validate(&self, registry: &ControlPayloadOwnerRegistry) -> UseResult<()> {
        let limits = capability_payload_contract(registry)?;
        self.binding.validate(registry)?;
        let payload_valid = match &self.payload {
            ControlCapabilityPayloadRestoreState::Absent => true,
            ControlCapabilityPayloadRestoreState::Archive {
                records,
                archive_bytes,
                archive_sha256,
            } => {
                *records > 0
                    && *records <= limits.max_files
                    && *archive_bytes > 0
                    && *archive_bytes <= limits.max_payload_bytes
                    && valid_sha256(archive_sha256)
            }
        };
        if self.schema != "a3s.use.control-capability-payload-restore-result.v1"
            || !valid_sha256(&self.owner_manifest_digest)
            || !valid_sha256(&self.inventory_digest)
            || !payload_valid
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_invalid(
                "The Capability payload restore result is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    fn validate_for_snapshot(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlCapabilityPayloadSnapshot,
    ) -> UseResult<()> {
        self.validate(registry)?;
        snapshot.validate(registry, &snapshot.manifest.binding)?;
        let matches = match (&self.payload, &snapshot.manifest.payload) {
            (
                ControlCapabilityPayloadRestoreState::Absent,
                ControlCapabilityPayloadState::Absent,
            ) => true,
            (
                ControlCapabilityPayloadRestoreState::Archive {
                    records,
                    archive_bytes,
                    archive_sha256,
                },
                ControlCapabilityPayloadState::Archive {
                    archive_bytes: expected_bytes,
                    archive_sha256: expected_digest,
                },
            ) => {
                *records == snapshot.manifest.entries.len() as u64
                    && *archive_bytes == *expected_bytes
                    && archive_sha256 == expected_digest
            }
            _ => false,
        };
        if self.binding != snapshot.manifest.binding
            || self.owner_manifest_digest != snapshot.manifest.descriptor_digest
            || self.inventory_digest != snapshot.manifest.inventory_digest
            || !matches
        {
            return Err(restore_invalid(
                "The Capability payload restore result differs from its snapshot.",
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
            inventory_digest: &'a str,
            payload: &'a ControlCapabilityPayloadRestoreState,
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            owner_manifest_digest: &self.owner_manifest_digest,
            inventory_digest: &self.inventory_digest,
            payload: &self.payload,
        })
        .map_err(|error| {
            capability_payload_error(format!(
                "Failed to encode the Capability payload restore descriptor: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(b"a3s.use.control-capability-payload-restore-result.v1\0");
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }

    pub(in crate::control_store) fn validate_for_registry(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        self.validate(registry)
    }
}

struct CapturedCapabilityPayload {
    payload: ControlCapabilityPayloadState,
    entries: Vec<ControlCapabilityPayloadEntry>,
    archive_path: Option<PathBuf>,
}

async fn snapshot_live(
    catalogs: &CapabilityGatewayCatalogStore,
    descriptors: &ControlCapabilityDescriptorSnapshotStore,
    maintenance: &StateMaintenanceGuard,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<CapturedCapabilityPayload> {
    let catalog_records = catalogs
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    let descriptor_records = descriptors
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    let mut entries = Vec::with_capacity(catalog_records.len() + descriptor_records.len());
    let mut total = 0_u64;
    for record in &catalog_records {
        let length = u64::try_from(record.bytes.len()).map_err(|_| {
            capability_payload_error("Capability payload record length overflowed.")
        })?;
        total = total.checked_add(length).ok_or_else(|| {
            capability_payload_error("Capability payload archive byte accounting overflowed.")
        })?;
        entries.push(ControlCapabilityPayloadEntry {
            kind: ControlCapabilityPayloadEntryKind::Catalog,
            digest: record.digest.clone(),
            length,
            sha256: digest_bytes(&record.bytes),
        });
    }
    for record in &descriptor_records {
        let length = u64::try_from(record.bytes.len()).map_err(|_| {
            capability_payload_error("Capability payload record length overflowed.")
        })?;
        total = total.checked_add(length).ok_or_else(|| {
            capability_payload_error("Capability payload archive byte accounting overflowed.")
        })?;
        entries.push(ControlCapabilityPayloadEntry {
            kind: ControlCapabilityPayloadEntryKind::DescriptorSnapshot,
            digest: record.digest.clone(),
            length,
            sha256: digest_bytes(&record.bytes),
        });
    }
    if entries.is_empty() {
        if path_exists(&destination).await? {
            return Err(capability_payload_error(
                "An absent Capability payload has an unexpected archive file.",
            ));
        }
        return Ok(CapturedCapabilityPayload {
            payload: ControlCapabilityPayloadState::Absent,
            entries: Vec::new(),
            archive_path: None,
        });
    }
    if entries.len() as u64 > limits.max_files || total > limits.max_payload_bytes {
        return Err(capability_payload_error(
            "The Capability payload exceeds its registered bounds.",
        ));
    }
    entries.sort_by(|left, right| left.sort_key().cmp(&right.sort_key()));
    let parent = destination.parent().ok_or_else(|| {
        capability_payload_error(
            "The Capability payload archive destination has no parent directory.",
        )
    })?;
    fs::create_dir_all(parent).await.map_err(|error| {
        capability_payload_io(format!("create Capability payload archive parent: {error}"))
    })?;
    let temporary_parent = parent.to_path_buf();
    let temporary = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".a3s-use-capability-payload-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!("join Capability payload archive staging: {error}"))
    })?
    .map_err(|error| {
        capability_payload_io(format!(
            "create Capability payload archive staging: {error}"
        ))
    })?;
    let writer_file = temporary.as_file().try_clone().map_err(|error| {
        capability_payload_io(format!("clone Capability payload archive handle: {error}"))
    })?;
    let mut writer = fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    let record_bytes = |kind: ControlCapabilityPayloadEntryKind,
                        digest: &str|
     -> UseResult<&[u8]> {
        match kind {
            ControlCapabilityPayloadEntryKind::Catalog => catalog_records
                .iter()
                .find(|record| record.digest == digest)
                .map(|record| record.bytes.as_slice())
                .ok_or_else(|| {
                    capability_payload_error("Capability payload archive inventory lost a catalog.")
                }),
            ControlCapabilityPayloadEntryKind::DescriptorSnapshot => descriptor_records
                .iter()
                .find(|record| record.digest == digest)
                .map(|record| record.bytes.as_slice())
                .ok_or_else(|| {
                    capability_payload_error(
                        "Capability payload archive inventory lost a descriptor snapshot.",
                    )
                }),
        }
    };
    for entry in &entries {
        let bytes = record_bytes(entry.kind, &entry.digest)?;
        writer.write_all(bytes).await.map_err(|error| {
            capability_payload_io(format!("write Capability payload archive: {error}"))
        })?;
        archive_digest.update(bytes);
    }
    writer.flush().await.map_err(|error| {
        capability_payload_io(format!("flush Capability payload archive: {error}"))
    })?;
    writer.sync_all().await.map_err(|error| {
        capability_payload_io(format!("sync Capability payload archive: {error}"))
    })?;
    drop(writer);
    let after_catalogs = catalogs
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    let after_descriptors = descriptors
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_capability_error)?;
    if after_catalogs != catalog_records || after_descriptors != descriptor_records {
        return Err(capability_payload_error(
            "Capability payload records changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!(
            "join Capability payload archive publication: {error}"
        ))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            capability_payload_error("The Capability payload archive destination already exists.")
        } else {
            capability_payload_io(format!("publish Capability payload archive: {error}"))
        }
    })?;
    if let Some(parent) = destination.parent() {
        sync_directory(parent).await?;
    }
    Ok(CapturedCapabilityPayload {
        payload: ControlCapabilityPayloadState::Archive {
            archive_bytes: total,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        entries,
        archive_path: Some(destination),
    })
}

async fn verify_archive(
    snapshot: &ControlCapabilityPayloadSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<()> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlCapabilityPayloadState::Absent, None) => Ok(()),
        (ControlCapabilityPayloadState::Archive { .. }, Some(path)) => {
            let _ = read_archive_records(path, snapshot).await?;
            Ok(())
        }
        _ => Err(capability_payload_error(
            "Capability payload archive presence differs from its snapshot manifest.",
        )),
    }
}

async fn read_archive_records(
    path: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<(
    Vec<CapabilityGatewayCatalogStoredRecord>,
    Vec<ControlCapabilityDescriptorSnapshotStoredRecord>,
)> {
    let (mut file, before) = open_owned_file(path).await?;
    let expected_bytes = match snapshot.manifest.payload {
        ControlCapabilityPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlCapabilityPayloadState::Absent => 0,
    };
    if before.len() != expected_bytes {
        return Err(capability_payload_error(
            "The Capability payload archive length differs from its manifest.",
        ));
    }
    let mut digest = Sha256::new();
    let mut catalogs = Vec::new();
    let mut descriptors = Vec::new();
    for entry in &snapshot.manifest.entries {
        let length = usize::try_from(entry.length).map_err(|_| {
            capability_payload_error("Capability payload archive record length overflowed.")
        })?;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes).await.map_err(|_| {
            capability_payload_error("The Capability payload archive is truncated.")
        })?;
        if digest_bytes(&bytes) != entry.sha256 || entry.sha256 != entry.digest {
            return Err(capability_payload_error(
                "A Capability payload archive record differs from its manifest digest.",
            ));
        }
        digest.update(&bytes);
        match entry.kind {
            ControlCapabilityPayloadEntryKind::Catalog => {
                catalogs.push(CapabilityGatewayCatalogStoredRecord {
                    digest: entry.digest.clone(),
                    bytes,
                });
            }
            ControlCapabilityPayloadEntryKind::DescriptorSnapshot => {
                descriptors.push(ControlCapabilityDescriptorSnapshotStoredRecord {
                    digest: entry.digest.clone(),
                    bytes,
                });
            }
        }
    }
    let mut trailing = [0_u8; 1];
    if file.read(&mut trailing).await.map_err(|error| {
        capability_payload_io(format!("read Capability payload archive tail: {error}"))
    })? != 0
    {
        return Err(capability_payload_error(
            "The Capability payload archive contains trailing bytes.",
        ));
    }
    if let ControlCapabilityPayloadState::Archive { archive_sha256, .. } =
        &snapshot.manifest.payload
    {
        if format!("sha256:{:x}", digest.finalize()) != *archive_sha256 {
            return Err(capability_payload_error(
                "The Capability payload archive digest differs from its manifest.",
            ));
        }
    }
    let after = file.metadata().await.map_err(|error| {
        capability_payload_io(format!("reinspect Capability payload archive: {error}"))
    })?;
    if !after.is_file()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(capability_payload_error(
            "The Capability payload archive changed during offline verification.",
        ));
    }
    Ok((catalogs, descriptors))
}

async fn stage_archive_file(
    source: &Path,
    staging: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<PathBuf> {
    let target = staging.join(ARCHIVE_FILE);
    let partial = staging.join(ARCHIVE_PARTIAL_FILE);
    let expected = read_archive_bytes(source, snapshot).await?;
    if let Some(existing) = optional_owned_file(&target).await? {
        if fs::read(&existing).await.map_err(|error| {
            capability_payload_io(format!("read staged Capability payload archive: {error}"))
        })? != expected
        {
            return Err(restore_invalid(
                "The staged Capability payload archive differs from its exact snapshot.",
            ));
        }
        return Ok(target);
    }
    if let Some(existing) = optional_owned_file(&partial).await? {
        let bytes = fs::read(&existing).await.map_err(|error| {
            capability_payload_io(format!("read partial Capability payload archive: {error}"))
        })?;
        if bytes == expected {
            publish_noclobber(partial, target.clone()).await?;
            return Ok(target);
        }
        if bytes.len() >= expected.len() {
            return Err(restore_invalid(
                "The partial Capability payload archive contains unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial).await.map_err(|error| {
            capability_payload_io(format!(
                "remove partial Capability payload archive: {error}"
            ))
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "create partial Capability payload archive: {error}"
            ))
        })?;
    file.write_all(&expected).await.map_err(|error| {
        capability_payload_io(format!("write partial Capability payload archive: {error}"))
    })?;
    file.flush().await.map_err(|error| {
        capability_payload_io(format!("flush partial Capability payload archive: {error}"))
    })?;
    file.sync_all().await.map_err(|error| {
        capability_payload_io(format!("sync partial Capability payload archive: {error}"))
    })?;
    drop(file);
    publish_noclobber(partial, target.clone()).await?;
    Ok(target)
}

async fn read_archive_bytes(
    source: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<Vec<u8>> {
    let bytes = fs::read(source).await.map_err(|error| {
        capability_payload_io(format!("read Capability payload archive source: {error}"))
    })?;
    let expected = match snapshot.manifest.payload {
        ControlCapabilityPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlCapabilityPayloadState::Absent => 0,
    };
    if bytes.len() as u64 != expected {
        return Err(capability_payload_error(
            "The Capability payload archive source has unexpected length.",
        ));
    }
    Ok(bytes)
}

async fn validate_candidate(
    candidate: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<()> {
    validate_inventory_at(candidate, snapshot).await
}

async fn validate_live_root(
    live: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<()> {
    validate_inventory_at(live, snapshot).await
}

async fn validate_inventory_at(
    root: &Path,
    snapshot: &ControlCapabilityPayloadSnapshot,
) -> UseResult<()> {
    let installation = &snapshot.manifest.binding.installation;
    let catalogs = CapabilityGatewayCatalogStore::inspect_records_at(
        &root.join(CATALOGS_DIRECTORY),
        installation,
    )
    .await
    .map_err(wrap_capability_error)?;
    let descriptors = ControlCapabilityDescriptorSnapshotStore::inspect_records_at(
        &root.join(DESCRIPTOR_SNAPSHOTS_DIRECTORY),
        installation,
    )
    .await
    .map_err(wrap_capability_error)?;
    let expected_catalogs = snapshot
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == ControlCapabilityPayloadEntryKind::Catalog)
        .collect::<Vec<_>>();
    let expected_descriptors = snapshot
        .manifest
        .entries
        .iter()
        .filter(|entry| entry.kind == ControlCapabilityPayloadEntryKind::DescriptorSnapshot)
        .collect::<Vec<_>>();
    if catalogs.len() != expected_catalogs.len()
        || descriptors.len() != expected_descriptors.len()
        || catalogs
            .iter()
            .zip(&expected_catalogs)
            .any(|(record, entry)| {
                record.digest != entry.digest
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
        || descriptors
            .iter()
            .zip(&expected_descriptors)
            .any(|(record, entry)| {
                record.digest != entry.digest
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
    {
        return Err(restore_invalid(
            "The Capability payload restore inventory differs from its exact snapshot.",
        ));
    }
    // Absent optional child directories are fine when that kind has no entries.
    if expected_catalogs.is_empty() {
        reject_unexpected_child(root, CATALOGS_DIRECTORY).await?;
    }
    if expected_descriptors.is_empty() {
        reject_unexpected_child(root, DESCRIPTOR_SNAPSHOTS_DIRECTORY).await?;
    }
    Ok(())
}

async fn reject_unexpected_child(root: &Path, name: &str) -> UseResult<()> {
    let path = root.join(name);
    match fs::symlink_metadata(&path).await {
        Ok(_) => Err(restore_invalid(
            "The Capability payload root contains an unexpected empty child directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload child directory: {error}"
        ))),
    }
}

async fn materialize_candidate(
    candidate: &Path,
    installation: &InstallationId,
    catalogs: &[CapabilityGatewayCatalogStoredRecord],
    descriptors: &[ControlCapabilityDescriptorSnapshotStoredRecord],
) -> UseResult<()> {
    ensure_owned_directory(candidate, candidate).await?;
    if !catalogs.is_empty() {
        let catalogs_root = candidate.join(CATALOGS_DIRECTORY);
        ensure_owned_directory(candidate, &catalogs_root).await?;
        CapabilityGatewayCatalogStore::materialize_records(&catalogs_root, installation, catalogs)
            .await
            .map_err(wrap_capability_error)?;
    }
    if !descriptors.is_empty() {
        let descriptors_root = candidate.join(DESCRIPTOR_SNAPSHOTS_DIRECTORY);
        ensure_owned_directory(candidate, &descriptors_root).await?;
        ControlCapabilityDescriptorSnapshotStore::materialize_records(
            &descriptors_root,
            installation,
            descriptors,
        )
        .await
        .map_err(wrap_capability_error)?;
    }
    Ok(())
}

async fn inspect_live_root(state_root: &Path) -> UseResult<Option<PathBuf>> {
    let path = state_root.join(CANDIDATE_DIRECTORY);
    match fs::symlink_metadata(&path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(Some(path))
        }
        Ok(_) => Err(restore_invalid(
            "The live Capability payload root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(capability_payload_io(format!(
            "inspect live Capability payload root: {error}"
        ))),
    }
}

async fn owned_directory(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata) && metadata.is_dir() =>
        {
            Ok(true)
        }
        Ok(_) => Err(restore_invalid(
            "A Capability payload restore directory is not owned.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload restore directory: {error}"
        ))),
    }
}

async fn open_owned_file(path: &Path) -> UseResult<(fs::File, std::fs::Metadata)> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        capability_payload_io(format!("inspect Capability payload archive: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
    {
        return Err(capability_payload_error(
            "The Capability payload archive is not an owned regular file.",
        ));
    }
    let file = fs::File::open(path).await.map_err(|error| {
        capability_payload_io(format!("open Capability payload archive: {error}"))
    })?;
    Ok((file, metadata))
}

async fn optional_owned_file(path: &Path) -> UseResult<Option<PathBuf>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            Ok(Some(path.to_path_buf()))
        }
        Ok(_) => Err(restore_invalid(
            "A Capability payload restore archive path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload restore archive: {error}"
        ))),
    }
}

async fn staged_archive(staging: &Path) -> UseResult<PathBuf> {
    optional_owned_file(&staging.join(ARCHIVE_FILE))
        .await?
        .ok_or_else(|| restore_invalid("The Capability payload restore archive is missing."))
}

async fn validate_staging_entries(staging: &Path) -> UseResult<()> {
    validate_directory(staging).await?;
    let mut entries = fs::read_dir(staging).await.map_err(|error| {
        capability_payload_io(format!("read Capability payload restore staging: {error}"))
    })?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        capability_payload_io(format!(
            "read Capability payload restore staging entry: {error}"
        ))
    })? {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Capability payload restore staging names must be valid UTF-8.")
        })?;
        let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
            capability_payload_io(format!(
                "inspect Capability payload restore staging entry: {error}"
            ))
        })?;
        let is_candidate = name == CANDIDATE_DIRECTORY;
        let is_file = matches!(
            name.as_str(),
            ARCHIVE_FILE | ARCHIVE_PARTIAL_FILE | ACTIVATION_FILE | ACTIVATION_PARTIAL_FILE
        );
        if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
            || (is_candidate && !metadata.is_dir())
            || (!is_candidate && (!is_file || !metadata.is_file()))
        {
            return Err(restore_invalid(
                "Capability payload restore staging contains an unowned entry.",
            ));
        }
    }
    Ok(())
}

async fn recover_activation_marker(staging: &Path, expected: &[u8]) -> UseResult<bool> {
    let marker = staging.join(ACTIVATION_FILE);
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker_length = optional_owned_file_length(&marker).await?;
    let partial_length = optional_owned_file_length(&partial).await?;
    if marker_length.is_some() && partial_length.is_some() {
        return Err(restore_invalid(
            "The Capability payload activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_owned_file(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The Capability payload activation marker differs from its exact snapshot.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        fs::remove_file(&partial).await.map_err(|error| {
            capability_payload_io(format!(
                "remove incomplete Capability payload activation marker: {error}"
            ))
        })?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_owned_file(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A staged Capability payload activation marker has unexpected complete bytes.",
        ));
    }
    publish_noclobber(partial, marker.clone()).await?;
    sync_directory(staging).await?;
    Ok(true)
}

async fn create_activation_marker(staging: &Path, expected: &[u8]) -> UseResult<()> {
    if recover_activation_marker(staging, expected).await? {
        return Ok(());
    }
    let partial = staging.join(ACTIVATION_PARTIAL_FILE);
    let marker = staging.join(ACTIVATION_FILE);
    let mut output = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "create Capability payload activation marker: {error}"
            ))
        })?;
    output.write_all(expected).await.map_err(|error| {
        capability_payload_io(format!(
            "write Capability payload activation marker: {error}"
        ))
    })?;
    output.flush().await.map_err(|error| {
        capability_payload_io(format!(
            "flush Capability payload activation marker: {error}"
        ))
    })?;
    output.sync_all().await.map_err(|error| {
        capability_payload_io(format!(
            "sync Capability payload activation marker: {error}"
        ))
    })?;
    drop(output);
    if read_owned_file(&partial, expected.len() as u64).await? != expected {
        return Err(restore_invalid(
            "The Capability payload activation marker changed before publication.",
        ));
    }
    publish_noclobber(partial, marker).await?;
    sync_directory(staging).await
}

async fn optional_owned_file_length(path: &Path) -> UseResult<Option<u64>> {
    match fs::symlink_metadata(path).await {
        Ok(metadata)
            if !a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
                && metadata.is_file() =>
        {
            if metadata.len() == 0 || metadata.len() > MAX_ACTIVATION_BYTES {
                return Err(restore_invalid(
                    "The Capability payload activation marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "The Capability payload activation marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload activation marker: {error}"
        ))),
    }
}

async fn read_owned_file(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    let bytes = fs::read(path).await.map_err(|error| {
        capability_payload_io(format!(
            "read Capability payload activation marker: {error}"
        ))
    })?;
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        capability_payload_io(format!(
            "reinspect Capability payload activation marker: {error}"
        ))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != expected_length
        || bytes.len() as u64 != expected_length
    {
        return Err(restore_invalid(
            "The Capability payload activation marker changed while it was read.",
        ));
    }
    Ok(bytes)
}

async fn require_empty_staging(staging: &Path) -> UseResult<()> {
    validate_staging_entries(staging).await?;
    let mut entries = fs::read_dir(staging).await.map_err(|error| {
        capability_payload_io(format!("read absent Capability payload staging: {error}"))
    })?;
    if entries
        .next_entry()
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "read absent Capability payload staging entry: {error}"
            ))
        })?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent Capability payload snapshot has unexpected staged state.",
        ));
    }
    Ok(())
}

async fn ensure_owned_directory(root: &Path, target: &Path) -> UseResult<()> {
    if target == root || !target.starts_with(root) {
        return Err(restore_invalid(
            "A Capability payload restore directory escapes its state root.",
        ));
    }
    validate_directory(root).await?;
    let relative = target.strip_prefix(root).map_err(|_| {
        restore_invalid("A Capability payload restore directory is not state-owned.")
    })?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Capability payload restore directory is not normalized.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(capability_payload_io(format!(
                    "create Capability payload restore directory: {error}"
                )))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        capability_payload_io(format!("inspect Capability payload directory: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Capability payload restore path is not an owned directory.",
        ));
    }
    Ok(())
}

fn validate_staging_location(state_root: &Path, staging: &Path) -> UseResult<()> {
    if staging == state_root || !staging.starts_with(state_root) {
        return Err(restore_invalid(
            "Capability payload restore staging escapes the target state root.",
        ));
    }
    let relative = staging
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("Capability payload restore staging is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "Capability payload restore staging is not normalized.",
        ));
    }
    if staging.starts_with(state_root.join(CANDIDATE_DIRECTORY)) {
        return Err(restore_invalid(
            "The Capability payload candidate cannot be staged inside its live root.",
        ));
    }
    Ok(())
}

async fn publish_directory(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let target_for_worker = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target_for_worker)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!("join Capability payload root publication: {error}"))
    })?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            capability_payload_io(format!("publish Capability payload root: {error}"))
        }
    })?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn publish_noclobber(source: PathBuf, target: PathBuf) -> UseResult<()> {
    let target_for_worker = target.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_temporary_noclobber_blocking(source, &target_for_worker)
    })
    .await
    .map_err(|error| {
        capability_payload_io(format!(
            "join Capability payload archive publication: {error}"
        ))
    })?
    .map_err(|error| {
        capability_payload_io(format!("publish Capability payload archive: {error}"))
    })?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn path_exists(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(capability_payload_io(format!(
            "inspect Capability payload path: {error}"
        ))),
    }
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| {
            capability_payload_io(format!(
                "open Capability payload directory for sync: {error}"
            ))
        })?
        .sync_all()
        .await
        .map_err(|error| {
            capability_payload_io(format!("sync Capability payload directory: {error}"))
        })
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn inventory_digest(
    installation: &InstallationId,
    entries: &[ControlCapabilityPayloadEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ControlCapabilityPayloadEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| {
        capability_payload_error(format!(
            "Failed to encode the Capability payload inventory: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    digest.update(INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn activation_bytes(snapshot: &ControlCapabilityPayloadSnapshot) -> UseResult<Vec<u8>> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Activation<'a> {
        schema: &'static str,
        binding: &'a ControlPayloadSnapshotBinding,
        owner_manifest_digest: &'a str,
        inventory_digest: &'a str,
    }
    let bytes = canonical_json(&Activation {
        schema: ACTIVATION_SCHEMA,
        binding: &snapshot.manifest.binding,
        owner_manifest_digest: &snapshot.manifest.descriptor_digest,
        inventory_digest: &snapshot.manifest.inventory_digest,
    })
    .map_err(|error| {
        capability_payload_error(format!(
            "Failed to encode the Capability payload activation marker: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ACTIVATION_BYTES {
        return Err(capability_payload_error(
            "The Capability payload activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn capability_payload_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::CapabilityPayload)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(capability_payload_error(
            "The Capability payload owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_CAPABILITY_PAYLOAD_SNAPSHOT_SCHEMA {
        return Err(capability_payload_error(
            "The Capability payload owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

fn wrap_capability_error(error: UseError) -> UseError {
    capability_payload_error(format!(
        "Capability payload store validation failed: {}",
        error.message
    ))
}

fn capability_payload_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.capability_payload_snapshot_invalid",
        message,
    )
}

fn capability_payload_io(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.capability_payload_snapshot_io", message)
}

fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.capability_payload_restore_invalid",
        message,
    )
}

fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.capability_payload_restore_target_not_empty",
        "The clean-target Capability payload restore refuses to merge or replace an existing root.",
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ControlCapabilityPayloadSnapshot>();
    assert_send_sync::<VerifiedControlCapabilityPayloadSnapshot>();
    assert_send_sync::<StagedControlCapabilityPayloadRestore>();
};
