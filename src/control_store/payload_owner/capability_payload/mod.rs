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
pub(super) const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-capability-payload-snapshot.v1\0";
pub(super) const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-capability-payload-inventory.v1\0";
pub(super) const ARCHIVE_FILE: &str = "capability-payload.archive";
pub(super) const ARCHIVE_PARTIAL_FILE: &str = "capability-payload.archive.partial";
pub(super) const ACTIVATION_FILE: &str = "capability-payload.activating.json";
pub(super) const ACTIVATION_PARTIAL_FILE: &str = "capability-payload.activating.json.partial";
pub(super) const CANDIDATE_DIRECTORY: &str = "capability-gateway";
pub(super) const CATALOGS_DIRECTORY: &str = "catalogs";
pub(super) const DESCRIPTOR_SNAPSHOTS_DIRECTORY: &str = "descriptor-snapshots";
pub(super) const ACTIVATION_SCHEMA: &str = "a3s.use.control-capability-payload-activation.v1";
pub(super) const MAX_ACTIVATION_BYTES: u64 = 16 * 1024;
pub(super) const MAX_ARCHIVE_RECORD_BYTES: u64 = if MAX_CAPABILITY_GATEWAY_CATALOG_BYTES
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
        // `digest` is the owner-native content identity (catalog descriptor
        // digest or domain-separated descriptor-snapshot digest). `sha256` is
        // the plain hash of the archived bytes and may differ when the owner
        // identity is domain-separated.
        if !valid_sha256(&self.digest)
            || self.length == 0
            || self.length > MAX_ARCHIVE_RECORD_BYTES
            || !valid_sha256(&self.sha256)
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


mod helpers;
mod archive;
mod filesystem;
use helpers::*;
use archive::*;
use filesystem::*;
