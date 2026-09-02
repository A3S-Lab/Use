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
const SNAPSHOT_DOMAIN: &[u8] = b"a3s.use.control-runtime-plan-payload-snapshot.v1\0";
const INVENTORY_DOMAIN: &[u8] = b"a3s.use.control-runtime-plan-payload-inventory.v1\0";
const ARCHIVE_FILE: &str = "runtime-plans.archive";
const ARCHIVE_PARTIAL_FILE: &str = "runtime-plans.archive.partial";
const ACTIVATION_FILE: &str = "runtime-plans.activating.json";
const ACTIVATION_PARTIAL_FILE: &str = "runtime-plans.activating.json.partial";
const CANDIDATE_DIRECTORY: &str = "runtime-plans";
const ACTIVATION_SCHEMA: &str = "a3s.use.control-runtime-plan-payload-activation.v1";
const MAX_ACTIVATION_BYTES: u64 = 16 * 1024;
const MAX_ARCHIVE_RECORD_BYTES: u64 =
    crate::plugin_runtime::MAX_RUNTIME_SURFACE_PLAN_RECORD_BYTES as u64;

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

struct CapturedRuntimePlans {
    payload: ControlRuntimePlanPayloadState,
    entries: Vec<ControlRuntimePlanPayloadEntry>,
    archive_path: Option<PathBuf>,
}

async fn snapshot_live(
    store: &RuntimeSurfacePlanStore,
    maintenance: &StateMaintenanceGuard,
    destination: PathBuf,
    limits: ControlPayloadOwnerLimits,
) -> UseResult<CapturedRuntimePlans> {
    let records = store
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_plan_error)?;
    if records.is_empty() {
        if path_exists(&destination).await? {
            return Err(runtime_plan_error(
                "An absent Runtime plan payload has an unexpected archive file.",
            ));
        }
        return Ok(CapturedRuntimePlans {
            payload: ControlRuntimePlanPayloadState::Absent,
            entries: Vec::new(),
            archive_path: None,
        });
    }
    let mut entries = Vec::with_capacity(records.len());
    let mut total = 0_u64;
    for record in &records {
        let key_digest = record.key.descriptor_digest().map_err(wrap_plan_error)?;
        let length = u64::try_from(record.bytes.len())
            .map_err(|_| runtime_plan_error("Runtime plan record length overflowed."))?;
        total = total.checked_add(length).ok_or_else(|| {
            runtime_plan_error("Runtime plan archive byte accounting overflowed.")
        })?;
        entries.push(ControlRuntimePlanPayloadEntry {
            key: record.key.clone(),
            key_digest,
            length,
            sha256: digest_bytes(&record.bytes),
        });
    }
    if entries.len() as u64 > limits.max_files || total > limits.max_payload_bytes {
        return Err(runtime_plan_error(
            "The Runtime plan payload exceeds its registered bounds.",
        ));
    }
    entries.sort_by(|left, right| left.key_digest.cmp(&right.key_digest));
    let parent = destination.parent().ok_or_else(|| {
        runtime_plan_error("The Runtime plan archive destination has no parent directory.")
    })?;
    fs::create_dir_all(parent)
        .await
        .map_err(|error| runtime_plan_io(format!("create Runtime plan archive parent: {error}")))?;
    let temporary_parent = parent.to_path_buf();
    let temporary = tokio::task::spawn_blocking(move || {
        tempfile::Builder::new()
            .prefix(".a3s-use-runtime-plans-")
            .suffix(".tmp")
            .tempfile_in(temporary_parent)
    })
    .await
    .map_err(|error| runtime_plan_io(format!("join Runtime plan archive staging: {error}")))?
    .map_err(|error| runtime_plan_io(format!("create Runtime plan archive staging: {error}")))?;
    let writer_file = temporary
        .as_file()
        .try_clone()
        .map_err(|error| runtime_plan_io(format!("clone Runtime plan archive handle: {error}")))?;
    let mut writer = fs::File::from_std(writer_file);
    let mut archive_digest = Sha256::new();
    for entry in &entries {
        let record = records
            .iter()
            .find(|record| {
                record.key.descriptor_digest().ok().as_deref() == Some(&entry.key_digest)
            })
            .ok_or_else(|| runtime_plan_error("Runtime plan archive inventory lost a record."))?;
        writer
            .write_all(&record.bytes)
            .await
            .map_err(|error| runtime_plan_io(format!("write Runtime plan archive: {error}")))?;
        archive_digest.update(&record.bytes);
    }
    writer
        .flush()
        .await
        .map_err(|error| runtime_plan_io(format!("flush Runtime plan archive: {error}")))?;
    writer
        .sync_all()
        .await
        .map_err(|error| runtime_plan_io(format!("sync Runtime plan archive: {error}")))?;
    drop(writer);
    let after = store
        .snapshot_records_under_maintenance(maintenance)
        .await
        .map_err(wrap_plan_error)?;
    if after != records {
        return Err(runtime_plan_error(
            "Runtime plan records changed during snapshot creation.",
        ));
    }
    let target = destination.clone();
    tokio::task::spawn_blocking(move || {
        a3s_use_extension::persist_named_temporary_noclobber_blocking(temporary, &target)
    })
    .await
    .map_err(|error| runtime_plan_io(format!("join Runtime plan archive publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            runtime_plan_error("The Runtime plan archive destination already exists.")
        } else {
            runtime_plan_io(format!("publish Runtime plan archive: {error}"))
        }
    })?;
    if let Some(parent) = destination.parent() {
        sync_directory(parent).await?;
    }
    Ok(CapturedRuntimePlans {
        payload: ControlRuntimePlanPayloadState::Archive {
            archive_bytes: total,
            archive_sha256: format!("sha256:{:x}", archive_digest.finalize()),
        },
        entries,
        archive_path: Some(destination),
    })
}

async fn verify_archive(
    snapshot: &ControlRuntimePlanPayloadSnapshot,
    archive_path: Option<&Path>,
) -> UseResult<()> {
    match (&snapshot.manifest.payload, archive_path) {
        (ControlRuntimePlanPayloadState::Absent, None) => Ok(()),
        (ControlRuntimePlanPayloadState::Archive { .. }, Some(path)) => {
            let _ = read_archive_records(path, snapshot).await?;
            Ok(())
        }
        _ => Err(runtime_plan_error(
            "Runtime plan archive presence differs from its snapshot manifest.",
        )),
    }
}

async fn read_archive_records(
    path: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<Vec<RuntimeSurfacePlanStoredRecord>> {
    let (mut file, before) = open_owned_file(path).await?;
    let expected_bytes = match snapshot.manifest.payload {
        ControlRuntimePlanPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlRuntimePlanPayloadState::Absent => 0,
    };
    if before.len() != expected_bytes {
        return Err(runtime_plan_error(
            "The Runtime plan archive length differs from its manifest.",
        ));
    }
    let mut digest = Sha256::new();
    let mut records = Vec::with_capacity(snapshot.manifest.entries.len());
    for entry in &snapshot.manifest.entries {
        let length = usize::try_from(entry.length)
            .map_err(|_| runtime_plan_error("Runtime plan archive record length overflowed."))?;
        let mut bytes = vec![0_u8; length];
        file.read_exact(&mut bytes)
            .await
            .map_err(|_| runtime_plan_error("The Runtime plan archive is truncated."))?;
        if digest_bytes(&bytes) != entry.sha256 {
            return Err(runtime_plan_error(
                "A Runtime plan archive record differs from its manifest digest.",
            ));
        }
        let (key, _) =
            RuntimeSurfacePlanStore::decode_record_bytes(&bytes).map_err(wrap_plan_error)?;
        if key != entry.key || key.descriptor_digest().map_err(wrap_plan_error)? != entry.key_digest
        {
            return Err(runtime_plan_error(
                "A Runtime plan archive record differs from its addressed key.",
            ));
        }
        digest.update(&bytes);
        records.push(RuntimeSurfacePlanStoredRecord { key, bytes });
    }
    let mut trailing = [0_u8; 1];
    if file
        .read(&mut trailing)
        .await
        .map_err(|error| runtime_plan_io(format!("read Runtime plan archive tail: {error}")))?
        != 0
    {
        return Err(runtime_plan_error(
            "The Runtime plan archive contains trailing bytes.",
        ));
    }
    if let ControlRuntimePlanPayloadState::Archive { archive_sha256, .. } =
        &snapshot.manifest.payload
    {
        if format!("sha256:{:x}", digest.finalize()) != *archive_sha256 {
            return Err(runtime_plan_error(
                "The Runtime plan archive digest differs from its manifest.",
            ));
        }
    }
    let after = file
        .metadata()
        .await
        .map_err(|error| runtime_plan_io(format!("reinspect Runtime plan archive: {error}")))?;
    if !after.is_file()
        || after.len() != before.len()
        || after.modified().ok() != before.modified().ok()
    {
        return Err(runtime_plan_error(
            "The Runtime plan archive changed during offline verification.",
        ));
    }
    Ok(records)
}

async fn stage_archive_file(
    source: &Path,
    staging: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<PathBuf> {
    let target = staging.join(ARCHIVE_FILE);
    let partial = staging.join(ARCHIVE_PARTIAL_FILE);
    let expected = read_archive_bytes(source, snapshot).await?;
    if let Some(existing) = optional_owned_file(&target).await? {
        if fs::read(&existing).await.map_err(|error| {
            runtime_plan_io(format!("read staged Runtime plan archive: {error}"))
        })? != expected
        {
            return Err(restore_invalid(
                "The staged Runtime plan archive differs from its exact snapshot.",
            ));
        }
        return Ok(target);
    }
    if let Some(existing) = optional_owned_file(&partial).await? {
        let bytes = fs::read(&existing).await.map_err(|error| {
            runtime_plan_io(format!("read partial Runtime plan archive: {error}"))
        })?;
        if bytes == expected {
            publish_noclobber(partial, target.clone()).await?;
            return Ok(target);
        }
        if bytes.len() >= expected.len() {
            return Err(restore_invalid(
                "The partial Runtime plan archive contains unexpected complete bytes.",
            ));
        }
        fs::remove_file(&partial).await.map_err(|error| {
            runtime_plan_io(format!("remove partial Runtime plan archive: {error}"))
        })?;
    }
    let mut file = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&partial)
        .await
        .map_err(|error| {
            runtime_plan_io(format!("create partial Runtime plan archive: {error}"))
        })?;
    file.write_all(&expected)
        .await
        .map_err(|error| runtime_plan_io(format!("write partial Runtime plan archive: {error}")))?;
    file.flush()
        .await
        .map_err(|error| runtime_plan_io(format!("flush partial Runtime plan archive: {error}")))?;
    file.sync_all()
        .await
        .map_err(|error| runtime_plan_io(format!("sync partial Runtime plan archive: {error}")))?;
    drop(file);
    publish_noclobber(partial, target.clone()).await?;
    Ok(target)
}

async fn read_archive_bytes(
    source: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<Vec<u8>> {
    let bytes = fs::read(source)
        .await
        .map_err(|error| runtime_plan_io(format!("read Runtime plan archive source: {error}")))?;
    let expected = match snapshot.manifest.payload {
        ControlRuntimePlanPayloadState::Archive { archive_bytes, .. } => archive_bytes,
        ControlRuntimePlanPayloadState::Absent => 0,
    };
    if bytes.len() as u64 != expected {
        return Err(runtime_plan_error(
            "The Runtime plan archive source has unexpected length.",
        ));
    }
    Ok(bytes)
}

async fn validate_candidate(
    candidate: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<()> {
    let records = RuntimeSurfacePlanStore::inspect_exact_records_at(
        candidate,
        &snapshot.manifest.binding.installation,
    )
    .await
    .map_err(wrap_plan_error)?;
    if records.len() != snapshot.manifest.entries.len()
        || records
            .iter()
            .zip(&snapshot.manifest.entries)
            .any(|(record, entry)| {
                record.key != entry.key
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
    {
        return Err(restore_invalid(
            "The Runtime plan restore candidate differs from its exact snapshot inventory.",
        ));
    }
    Ok(())
}

async fn validate_live_root(
    live: &Path,
    snapshot: &ControlRuntimePlanPayloadSnapshot,
) -> UseResult<()> {
    let records =
        RuntimeSurfacePlanStore::inspect_records_at(live, &snapshot.manifest.binding.installation)
            .await
            .map_err(wrap_plan_error)?;
    if records.len() != snapshot.manifest.entries.len()
        || records
            .iter()
            .zip(&snapshot.manifest.entries)
            .any(|(record, entry)| {
                record.key != entry.key
                    || record.bytes.len() as u64 != entry.length
                    || digest_bytes(&record.bytes) != entry.sha256
            })
    {
        return Err(restore_invalid(
            "The live Runtime plan root differs from its exact snapshot inventory.",
        ));
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
            "The live Runtime plan root is not an owned directory.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect live Runtime plan root: {error}"
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
            "A Runtime plan restore directory is not owned.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan restore directory: {error}"
        ))),
    }
}

async fn open_owned_file(path: &Path) -> UseResult<(fs::File, std::fs::Metadata)> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| runtime_plan_io(format!("inspect Runtime plan archive: {error}")))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() == 0
    {
        return Err(runtime_plan_error(
            "The Runtime plan archive is not an owned regular file.",
        ));
    }
    let file = fs::File::open(path)
        .await
        .map_err(|error| runtime_plan_io(format!("open Runtime plan archive: {error}")))?;
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
            "A Runtime plan restore archive path is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan restore archive: {error}"
        ))),
    }
}

async fn staged_archive(staging: &Path) -> UseResult<PathBuf> {
    optional_owned_file(&staging.join(ARCHIVE_FILE))
        .await?
        .ok_or_else(|| restore_invalid("The Runtime plan restore archive is missing."))
}

async fn validate_staging_entries(staging: &Path) -> UseResult<()> {
    validate_directory(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| runtime_plan_io(format!("read Runtime plan restore staging: {error}")))?;
    while let Some(entry) = entries.next_entry().await.map_err(|error| {
        runtime_plan_io(format!("read Runtime plan restore staging entry: {error}"))
    })? {
        let name = entry.file_name().into_string().map_err(|_| {
            restore_invalid("Runtime plan restore staging names must be valid UTF-8.")
        })?;
        let metadata = fs::symlink_metadata(entry.path()).await.map_err(|error| {
            runtime_plan_io(format!(
                "inspect Runtime plan restore staging entry: {error}"
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
                "Runtime plan restore staging contains an unowned entry.",
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
            "The Runtime plan activation marker state is ambiguous.",
        ));
    }
    if let Some(length) = marker_length {
        if length != expected.len() as u64 || read_owned_file(&marker, length).await? != expected {
            return Err(restore_invalid(
                "The Runtime plan activation marker differs from its exact snapshot.",
            ));
        }
        return Ok(true);
    }
    let Some(length) = partial_length else {
        return Ok(false);
    };
    if length < expected.len() as u64 {
        fs::remove_file(&partial).await.map_err(|error| {
            runtime_plan_io(format!(
                "remove incomplete Runtime plan activation marker: {error}"
            ))
        })?;
        sync_directory(staging).await?;
        return Ok(false);
    }
    if length != expected.len() as u64 || read_owned_file(&partial, length).await? != expected {
        return Err(restore_invalid(
            "A staged Runtime plan activation marker has unexpected complete bytes.",
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
            runtime_plan_io(format!("create Runtime plan activation marker: {error}"))
        })?;
    output.write_all(expected).await.map_err(|error| {
        runtime_plan_io(format!("write Runtime plan activation marker: {error}"))
    })?;
    output.flush().await.map_err(|error| {
        runtime_plan_io(format!("flush Runtime plan activation marker: {error}"))
    })?;
    output.sync_all().await.map_err(|error| {
        runtime_plan_io(format!("sync Runtime plan activation marker: {error}"))
    })?;
    drop(output);
    if read_owned_file(&partial, expected.len() as u64).await? != expected {
        return Err(restore_invalid(
            "The Runtime plan activation marker changed before publication.",
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
                    "The Runtime plan activation marker exceeds its byte bound.",
                ));
            }
            Ok(Some(metadata.len()))
        }
        Ok(_) => Err(restore_invalid(
            "The Runtime plan activation marker is not an owned regular file.",
        )),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan activation marker: {error}"
        ))),
    }
}

async fn read_owned_file(path: &Path, expected_length: u64) -> UseResult<Vec<u8>> {
    let bytes = fs::read(path).await.map_err(|error| {
        runtime_plan_io(format!("read Runtime plan activation marker: {error}"))
    })?;
    let metadata = fs::symlink_metadata(path).await.map_err(|error| {
        runtime_plan_io(format!("reinspect Runtime plan activation marker: {error}"))
    })?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata)
        || !metadata.is_file()
        || metadata.len() != expected_length
        || bytes.len() as u64 != expected_length
    {
        return Err(restore_invalid(
            "The Runtime plan activation marker changed while it was read.",
        ));
    }
    Ok(bytes)
}

async fn require_empty_staging(staging: &Path) -> UseResult<()> {
    validate_staging_entries(staging).await?;
    let mut entries = fs::read_dir(staging)
        .await
        .map_err(|error| runtime_plan_io(format!("read absent Runtime plan staging: {error}")))?;
    if entries
        .next_entry()
        .await
        .map_err(|error| {
            runtime_plan_io(format!("read absent Runtime plan staging entry: {error}"))
        })?
        .is_some()
    {
        return Err(restore_invalid(
            "An absent Runtime plan snapshot has unexpected staged state.",
        ));
    }
    Ok(())
}

async fn ensure_owned_directory(root: &Path, target: &Path) -> UseResult<()> {
    if target == root || !target.starts_with(root) {
        return Err(restore_invalid(
            "A Runtime plan restore directory escapes its state root.",
        ));
    }
    validate_directory(root).await?;
    let relative = target
        .strip_prefix(root)
        .map_err(|_| restore_invalid("A Runtime plan restore directory is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "A Runtime plan restore directory is not normalized.",
        ));
    }
    let mut current = root.to_path_buf();
    for component in relative.components() {
        current.push(component.as_os_str());
        match fs::create_dir(&current).await {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => {
                return Err(runtime_plan_io(format!(
                    "create Runtime plan restore directory: {error}"
                )))
            }
        }
        validate_directory(&current).await?;
    }
    Ok(())
}

async fn validate_directory(path: &Path) -> UseResult<()> {
    let metadata = fs::symlink_metadata(path)
        .await
        .map_err(|error| runtime_plan_io(format!("inspect Runtime plan directory: {error}")))?;
    if a3s_use_core::metadata_is_link_or_reparse_point(&metadata) || !metadata.is_dir() {
        return Err(restore_invalid(
            "A Runtime plan restore path is not an owned directory.",
        ));
    }
    Ok(())
}

fn validate_staging_location(state_root: &Path, staging: &Path) -> UseResult<()> {
    if staging == state_root || !staging.starts_with(state_root) {
        return Err(restore_invalid(
            "Runtime plan restore staging escapes the target state root.",
        ));
    }
    let relative = staging
        .strip_prefix(state_root)
        .map_err(|_| restore_invalid("Runtime plan restore staging is not state-owned."))?;
    if !relative
        .components()
        .all(|component| matches!(component, Component::Normal(_)))
    {
        return Err(restore_invalid(
            "Runtime plan restore staging is not normalized.",
        ));
    }
    if staging.starts_with(state_root.join(CANDIDATE_DIRECTORY)) {
        return Err(restore_invalid(
            "The Runtime plan candidate cannot be staged inside its live root.",
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
    .map_err(|error| runtime_plan_io(format!("join Runtime plan root publication: {error}")))?
    .map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            restore_target_not_empty()
        } else {
            runtime_plan_io(format!("publish Runtime plan root: {error}"))
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
    .map_err(|error| runtime_plan_io(format!("join Runtime plan archive publication: {error}")))?
    .map_err(|error| runtime_plan_io(format!("publish Runtime plan archive: {error}")))?;
    if let Some(parent) = target.parent() {
        sync_directory(parent).await?;
    }
    Ok(())
}

async fn path_exists(path: &Path) -> UseResult<bool> {
    match fs::symlink_metadata(path).await {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(runtime_plan_io(format!(
            "inspect Runtime plan path: {error}"
        ))),
    }
}

#[cfg(unix)]
async fn sync_directory(path: &Path) -> UseResult<()> {
    fs::File::open(path)
        .await
        .map_err(|error| runtime_plan_io(format!("open Runtime plan directory for sync: {error}")))?
        .sync_all()
        .await
        .map_err(|error| runtime_plan_io(format!("sync Runtime plan directory: {error}")))
}

#[cfg(not(unix))]
async fn sync_directory(_path: &Path) -> UseResult<()> {
    Ok(())
}

fn inventory_digest(
    installation: &InstallationId,
    entries: &[ControlRuntimePlanPayloadEntry],
) -> UseResult<String> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Inventory<'a> {
        installation: &'a InstallationId,
        entries: &'a [ControlRuntimePlanPayloadEntry],
    }
    let bytes = canonical_json(&Inventory {
        installation,
        entries,
    })
    .map_err(|error| {
        runtime_plan_error(format!(
            "Failed to encode the Runtime plan inventory: {error}"
        ))
    })?;
    let mut digest = Sha256::new();
    digest.update(INVENTORY_DOMAIN);
    digest.update(bytes);
    Ok(format!("sha256:{:x}", digest.finalize()))
}

fn activation_bytes(snapshot: &ControlRuntimePlanPayloadSnapshot) -> UseResult<Vec<u8>> {
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
        runtime_plan_error(format!(
            "Failed to encode the Runtime plan activation marker: {error}"
        ))
    })?;
    if bytes.is_empty() || bytes.len() as u64 > MAX_ACTIVATION_BYTES {
        return Err(runtime_plan_error(
            "The Runtime plan activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

fn digest_bytes(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn runtime_plan_contract(
    registry: &ControlPayloadOwnerRegistry,
) -> UseResult<ControlPayloadOwnerLimits> {
    registry.validate()?;
    let Some((schema, limits)) = registry
        .registration(ControlPayloadOwnerId::RuntimePlanPayload)
        .and_then(|registration| registration.snapshot_contract())
    else {
        return Err(runtime_plan_error(
            "The Runtime plan payload owner is not registered for snapshots.",
        ));
    };
    if schema != CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA {
        return Err(runtime_plan_error(
            "The Runtime plan payload owner schema is unsupported.",
        ));
    }
    Ok(limits)
}

fn wrap_plan_error(error: UseError) -> UseError {
    runtime_plan_error(format!(
        "Runtime plan store validation failed: {}",
        error.message
    ))
}

fn runtime_plan_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_snapshot_invalid",
        message,
    )
}

fn runtime_plan_io(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_snapshot_io",
        message,
    )
}

fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_restore_invalid",
        message,
    )
}

fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.runtime_plan_payload_restore_target_not_empty",
        "The clean-target Runtime plan restore refuses to merge or replace an existing root.",
    )
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ControlRuntimePlanPayloadSnapshot>();
    assert_send_sync::<VerifiedControlRuntimePlanPayloadSnapshot>();
    assert_send_sync::<StagedControlRuntimePlanPayloadRestore>();
};
