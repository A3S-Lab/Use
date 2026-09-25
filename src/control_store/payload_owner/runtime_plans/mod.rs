//! Snapshot and clean-target restore for the installation-scoped Runtime plan
//! payload.
//!
//! Runtime plans are immutable planning evidence. They are not a second source
//! of desired state: the Control export still decides which package and
//! provider identities are live. The owner only preserves the exact bytes that
//! a committed Runtime resolver needs after restart or restore.

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
use crate::control_store::model::valid_sha256;
use crate::plugin_runtime::{
    RuntimeSurfacePlanKey, RuntimeSurfacePlanStore, RuntimeSurfacePlanStoredRecord,
};

pub(in crate::control_store) const CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA: &str =
    "a3s.use.control-runtime-plan-payload-snapshot.v1";
pub(super) const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-runtime-plan-payload-snapshot.v1\0";
pub(super) const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-runtime-plan-payload-inventory.v1\0";
pub(super) const ARCHIVE_FILE: &str = "runtime-plans.archive";
pub(super) const ARCHIVE_PARTIAL_FILE: &str = "runtime-plans.archive.partial";
pub(super) const ACTIVATION_FILE: &str = "runtime-plans.activating.json";
pub(super) const ACTIVATION_PARTIAL_FILE: &str = "runtime-plans.activating.json.partial";
pub(super) const CANDIDATE_DIRECTORY: &str = "runtime-plans";
pub(super) const ACTIVATION_SCHEMA: &str = "a3s.use.control-runtime-plan-payload-activation.v1";
pub(super) const MAX_ACTIVATION_BYTES: u64 = 16 * 1024;
pub(super) const MAX_ARCHIVE_RECORD_BYTES: u64 =
    crate::plugin_runtime::MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES as u64;

pub(super) fn candidate_path(staging_directory: &Path) -> PathBuf {
    staging_directory.join(CANDIDATE_DIRECTORY)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlRuntimePlanPayloadState {
    Absent,
    Archive {
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRuntimePlanPayloadEntry {
    pub(in crate::control_store) key: RuntimeSurfacePlanKey,
    pub(in crate::control_store) key_digest: String,
    pub(in crate::control_store) length: u64,
    pub(in crate::control_store) sha256: String,
}

impl ControlRuntimePlanPayloadEntry {
    fn validate(&self, installation: &InstallationId) -> UseResult<()> {
        self.key.validate().map_err(wrap_plan_error)?;
        installation.ensure_same(&self.key.scope)?;
        if !valid_sha256(&self.key_digest)
            || self.key.descriptor_digest().map_err(wrap_plan_error)? != self.key_digest
            || self.length == 0
            || self.length > MAX_ARCHIVE_RECORD_BYTES
            || !valid_sha256(&self.sha256)
        {
            return Err(runtime_plan_error(
                "A Runtime plan payload entry is invalid or exceeds its bound.",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRuntimePlanPayloadSnapshotManifest {
    pub(in crate::control_store) schema: String,
    pub(in crate::control_store) binding: ControlPayloadSnapshotBinding,
    pub(in crate::control_store) created_at_ms: u64,
    pub(in crate::control_store) payload: ControlRuntimePlanPayloadState,
    pub(in crate::control_store) inventory_digest: String,
    pub(in crate::control_store) entries: Vec<ControlRuntimePlanPayloadEntry>,
    pub(in crate::control_store) descriptor_digest: String,
}

impl ControlRuntimePlanPayloadSnapshotManifest {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        binding: ControlPayloadSnapshotBinding,
        created_at_ms: u64,
        payload: ControlRuntimePlanPayloadState,
        entries: Vec<ControlRuntimePlanPayloadEntry>,
    ) -> UseResult<Self> {
        let inventory_digest = inventory_digest(&binding.installation, &entries)?;
        let mut manifest = Self {
            schema: CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA.to_owned(),
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
        let limits = runtime_plan_contract(registry)?;
        self.binding.validate(registry)?;
        if self.schema != CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA
            || &self.binding != expected_binding
            || self.created_at_ms == 0
            || !valid_sha256(&self.inventory_digest)
            || !valid_sha256(&self.descriptor_digest)
        {
            return Err(runtime_plan_error(
                "The Runtime plan payload manifest is invalid or was rebound.",
            ));
        }

        let mut previous: Option<&str> = None;
        let mut identities = BTreeSet::new();
        let mut byte_count = 0_u64;
        for entry in &self.entries {
            entry.validate(&self.binding.installation)?;
            if previous.is_some_and(|value| value >= entry.key_digest.as_str())
                || !identities.insert(entry.key_digest.as_str())
            {
                return Err(runtime_plan_error(
                    "Runtime plan payload entries are not sorted and unique.",
                ));
            }
            byte_count = byte_count.checked_add(entry.length).ok_or_else(|| {
                runtime_plan_error("Runtime plan payload byte accounting overflowed.")
            })?;
            previous = Some(entry.key_digest.as_str());
        }
        let file_count = u64::try_from(self.entries.len())
            .map_err(|_| runtime_plan_error("Runtime plan payload file accounting overflowed."))?;
        if file_count > limits.max_files || byte_count > limits.max_payload_bytes {
            return Err(runtime_plan_error(
                "The Runtime plan payload exceeds its registered bounds.",
            ));
        }
        match &self.payload {
            ControlRuntimePlanPayloadState::Absent => {
                if !self.entries.is_empty() || byte_count != 0 {
                    return Err(runtime_plan_error(
                        "An absent Runtime plan payload contains records.",
                    ));
                }
            }
            ControlRuntimePlanPayloadState::Archive {
                archive_bytes,
                archive_sha256,
            } => {
                if self.entries.is_empty()
                    || *archive_bytes != byte_count
                    || !valid_sha256(archive_sha256)
                {
                    return Err(runtime_plan_error(
                        "Runtime plan archive evidence differs from its entries.",
                    ));
                }
            }
        }
        if inventory_digest(&self.binding.installation, &self.entries)? != self.inventory_digest
            || self.expected_descriptor_digest()? != self.descriptor_digest
        {
            return Err(runtime_plan_error(
                "The Runtime plan payload manifest digest is inconsistent.",
            ));
        }
        let bytes = canonical_json(self).map_err(|error| {
            runtime_plan_error(format!(
                "Failed to encode the Runtime plan payload manifest: {error}"
            ))
        })?;
        if bytes.is_empty()
            || u64::try_from(bytes.len())
                .ok()
                .is_none_or(|length| length > limits.max_manifest_bytes)
        {
            return Err(runtime_plan_error(
                "The Runtime plan payload manifest exceeds its registered bound.",
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
            runtime_plan_error(format!(
                "Failed to encode the Runtime plan payload manifest: {error}"
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
            payload: &'a ControlRuntimePlanPayloadState,
            inventory_digest: &'a str,
            entries: &'a [ControlRuntimePlanPayloadEntry],
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
            runtime_plan_error(format!(
                "Failed to encode the Runtime plan payload descriptor: {error}"
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
pub(in crate::control_store) struct ControlRuntimePlanPayloadSnapshot {
    pub(in crate::control_store) manifest: ControlRuntimePlanPayloadSnapshotManifest,
    pub(in crate::control_store) receipt: ControlPayloadSnapshotReceipt,
}

impl ControlRuntimePlanPayloadSnapshot {
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
            .ok_or_else(|| runtime_plan_error("Runtime plan payload accounting overflowed."))?;
        if self.receipt.owner != ControlPayloadOwnerId::RuntimePlanPayload
            || self.receipt.owner_manifest_digest != self.manifest.descriptor_digest
            || self.receipt.inventory_digest != self.manifest.inventory_digest
            || self.receipt.manifest_bytes != manifest_bytes.len() as u64
            || self.receipt.file_count != file_count
            || self.receipt.byte_count != byte_count
        {
            return Err(runtime_plan_error(
                "The Runtime plan payload receipt differs from its manifest.",
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
    ) -> UseResult<VerifiedControlRuntimePlanPayloadSnapshot> {
        self.validate(registry, expected_binding)?;
        expected_binding.verify_control_export(registry, control_export)?;
        verify_archive(self, archive_path.as_deref()).await?;
        Ok(VerifiedControlRuntimePlanPayloadSnapshot {
            archive_path,
            registry: registry.clone(),
            snapshot: self.clone(),
        })
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct VerifiedControlRuntimePlanPayloadSnapshot {
    archive_path: Option<PathBuf>,
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlRuntimePlanPayloadSnapshot,
}

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlRuntimePlanPayloadRestore {
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlRuntimePlanPayloadSnapshot,
    state_root: PathBuf,
    staging_directory: PathBuf,
    candidate: Option<PathBuf>,
    activation_bytes: Vec<u8>,
}

impl ControlPayloadSnapshotSession {
    pub(in crate::control_store) async fn snapshot_runtime_plans(
        &self,
        destination: PathBuf,
        created_at_ms: u64,
    ) -> UseResult<ControlRuntimePlanPayloadSnapshot> {
        let limits = runtime_plan_contract(self.registry())?;
        let store = RuntimeSurfacePlanStore::new(
            self.state_root().to_path_buf(),
            self.binding().installation.clone(),
        )?;
        let captured = snapshot_live(&store, self.maintenance(), destination, limits).await?;
        let manifest = ControlRuntimePlanPayloadSnapshotManifest::new(
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
            .ok_or_else(|| runtime_plan_error("Runtime plan payload accounting overflowed."))?;
        let receipt = self.receipt(
            ControlPayloadOwnerId::RuntimePlanPayload,
            ControlPayloadSnapshotEvidence::new(
                manifest.descriptor_digest.clone(),
                manifest.inventory_digest.clone(),
                manifest_bytes.len() as u64,
                file_count,
                byte_count,
            ),
        )?;
        let snapshot = ControlRuntimePlanPayloadSnapshot { manifest, receipt };
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

impl VerifiedControlRuntimePlanPayloadSnapshot {
    pub(in crate::control_store) async fn stage_clean_restore_under_exclusive(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlRuntimePlanPayloadRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        validate_staging_location(&state_root, &staging_directory)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Runtime plan restore staging requires the exact target's exclusive maintenance guard.",
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
            (ControlRuntimePlanPayloadState::Absent, None) => {
                require_empty_staging(&staging_directory).await?;
                None
            }
            (ControlRuntimePlanPayloadState::Archive { .. }, Some(source)) => {
                let archive =
                    stage_archive_file(source, &staging_directory, &self.snapshot).await?;
                let records = read_archive_records(&archive, &self.snapshot).await?;
                let candidate = candidate_path(&staging_directory);
                // Once the owner root has been published, the candidate is moved out
                // of staging.  Reopening that crash window must not recreate an
                // empty candidate beside the already-live root; the activation
                // marker is the durable evidence that permits replay.
                if !activation_started || owned_directory(&candidate).await? {
                    ensure_owned_directory(&staging_directory, &candidate).await?;
                    RuntimeSurfacePlanStore::materialize_records(
                        &candidate,
                        &self.snapshot.manifest.binding.installation,
                        &records,
                    )
                    .await
                    .map_err(wrap_plan_error)?;
                }
                Some(candidate)
            }
            _ => {
                return Err(restore_invalid(
                    "The verified Runtime plan snapshot omitted or added archive bytes.",
                ))
            }
        };
        validate_staging_entries(&staging_directory).await?;
        if let Some(candidate) = &candidate {
            if owned_directory(candidate).await? {
                validate_candidate(candidate, &self.snapshot).await?;
            }
        }
        Ok(StagedControlRuntimePlanPayloadRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            state_root,
            staging_directory,
            candidate,
            activation_bytes,
        })
    }
}

impl StagedControlRuntimePlanPayloadRestore {
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
                "Runtime plan activation evidence exists before complete restore intent.",
            ));
        }
        match (&self.snapshot.manifest.payload, &self.candidate) {
            (ControlRuntimePlanPayloadState::Absent, None) => {
                require_empty_staging(&self.staging_directory).await
            }
            (ControlRuntimePlanPayloadState::Archive { .. }, Some(candidate)) => {
                let archive = staged_archive(&self.staging_directory).await?;
                verify_archive(&self.snapshot, Some(&archive)).await?;
                if !owned_directory(candidate).await? {
                    return Err(restore_invalid(
                        "The Runtime plan restore candidate disappeared before activation.",
                    ));
                }
                validate_candidate(candidate, &self.snapshot).await
            }
            _ => Err(restore_invalid(
                "The staged Runtime plan payload differs from its snapshot state.",
            )),
        }
    }

    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlRuntimePlanPayloadRestoreResult> {
        self.ensure_guard(maintenance)?;
        validate_staging_entries(&self.staging_directory).await?;
        match (&self.snapshot.manifest.payload, &self.candidate) {
            (ControlRuntimePlanPayloadState::Absent, None) => {
                require_empty_staging(&self.staging_directory).await?;
                if inspect_live_root(&self.state_root).await?.is_some() {
                    return Err(restore_target_not_empty());
                }
            }
            (ControlRuntimePlanPayloadState::Archive { .. }, Some(candidate)) => {
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
                    (false, None) => {
                        return Err(restore_invalid(
                            "The Runtime plan restore candidate disappeared before activation.",
                        ))
                    }
                }
                let live = inspect_live_root(&self.state_root).await?.ok_or_else(|| {
                    restore_invalid("The Runtime plan restore did not publish its live root.")
                })?;
                validate_live_root(&live, &self.snapshot).await?;
                if !recover_activation_marker(&self.staging_directory, &self.activation_bytes)
                    .await?
                {
                    return Err(restore_invalid(
                        "The Runtime plan activation marker disappeared before restore completion.",
                    ));
                }
            }
            _ => {
                return Err(restore_invalid(
                    "The staged Runtime plan payload differs from its snapshot state.",
                ))
            }
        }
        ControlRuntimePlanPayloadRestoreResult::new(&self.registry, &self.snapshot)
    }

    fn ensure_guard(&self, maintenance: &StateMaintenanceGuard) -> UseResult<()> {
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Runtime plan restore requires the exact target's exclusive maintenance guard.",
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
pub(in crate::control_store) enum ControlRuntimePlanPayloadRestoreState {
    Absent,
    Archive {
        records: u64,
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlRuntimePlanPayloadRestoreResult {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    inventory_digest: String,
    pub(in crate::control_store) payload: ControlRuntimePlanPayloadRestoreState,
    descriptor_digest: String,
}

impl ControlRuntimePlanPayloadRestoreResult {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlRuntimePlanPayloadSnapshot,
    ) -> UseResult<Self> {
        let payload = match &snapshot.manifest.payload {
            ControlRuntimePlanPayloadState::Absent => ControlRuntimePlanPayloadRestoreState::Absent,
            ControlRuntimePlanPayloadState::Archive {
                archive_bytes,
                archive_sha256,
            } => ControlRuntimePlanPayloadRestoreState::Archive {
                records: snapshot.manifest.entries.len() as u64,
                archive_bytes: *archive_bytes,
                archive_sha256: archive_sha256.clone(),
            },
        };
        let mut result = Self {
            schema: "a3s.use.control-runtime-plan-payload-restore-result.v1".to_owned(),
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
        let limits = runtime_plan_contract(registry)?;
        self.binding.validate(registry)?;
        let payload_valid = match &self.payload {
            ControlRuntimePlanPayloadRestoreState::Absent => true,
            ControlRuntimePlanPayloadRestoreState::Archive {
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
        if self.schema != "a3s.use.control-runtime-plan-payload-restore-result.v1"
            || !valid_sha256(&self.owner_manifest_digest)
            || !valid_sha256(&self.inventory_digest)
            || !payload_valid
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_invalid(
                "The Runtime plan restore result is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    fn validate_for_snapshot(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlRuntimePlanPayloadSnapshot,
    ) -> UseResult<()> {
        self.validate(registry)?;
        snapshot.validate(registry, &snapshot.manifest.binding)?;
        let matches = match (&self.payload, &snapshot.manifest.payload) {
            (
                ControlRuntimePlanPayloadRestoreState::Absent,
                ControlRuntimePlanPayloadState::Absent,
            ) => true,
            (
                ControlRuntimePlanPayloadRestoreState::Archive {
                    records,
                    archive_bytes,
                    archive_sha256,
                },
                ControlRuntimePlanPayloadState::Archive {
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
                "The Runtime plan restore result differs from its snapshot.",
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
            payload: &'a ControlRuntimePlanPayloadRestoreState,
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            owner_manifest_digest: &self.owner_manifest_digest,
            inventory_digest: &self.inventory_digest,
            payload: &self.payload,
        })
        .map_err(|error| {
            runtime_plan_error(format!(
                "Failed to encode the Runtime plan restore descriptor: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(b"a3s.use.control-runtime-plan-payload-restore-result.v1\0");
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
