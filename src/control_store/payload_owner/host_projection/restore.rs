use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    canonical_json, host_projection_contract, ControlHostProjectionSnapshot,
    ControlHostProjectionState, ControlPayloadOwnerRegistry, ControlPayloadSnapshotBinding,
    VerifiedControlHostProjectionSnapshot,
};
use crate::control_store::model::valid_sha256;

mod filesystem;

const RESTORE_RESULT_SCHEMA: &str = "a3s.use.control-host-projection-restore-result.v1";
const RESTORE_RESULT_DOMAIN: &[u8] = b"a3s.use.control-host-projection-restore-result.v1\0";
const ACTIVATION_SCHEMA: &str = "a3s.use.control-host-projection-restore-activation.v1";
const MAX_RESTORE_RESULT_BYTES: usize = 64 * 1024;
const MAX_ACTIVATION_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlHostProjectionRestoreState {
    Absent,
    Archive {
        source_records: u64,
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlHostProjectionRestoreResult {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    inventory_digest: String,
    pub(in crate::control_store) payload: ControlHostProjectionRestoreState,
    descriptor_digest: String,
}

impl ControlHostProjectionRestoreResult {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlHostProjectionSnapshot,
    ) -> UseResult<Self> {
        let payload = match &snapshot.manifest.payload {
            ControlHostProjectionState::Absent => ControlHostProjectionRestoreState::Absent,
            ControlHostProjectionState::Archive {
                archive_bytes,
                archive_sha256,
            } => ControlHostProjectionRestoreState::Archive {
                source_records: u64::try_from(snapshot.manifest.entries.len())
                    .map_err(|_| restore_invalid("The restored Host record count overflowed."))?,
                archive_bytes: *archive_bytes,
                archive_sha256: archive_sha256.clone(),
            },
        };
        let mut result = Self {
            schema: RESTORE_RESULT_SCHEMA.to_owned(),
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

    pub(in crate::control_store) fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
    ) -> UseResult<()> {
        let limits = host_projection_contract(registry)?;
        self.binding.validate(registry)?;
        let payload_valid = match &self.payload {
            ControlHostProjectionRestoreState::Absent => true,
            ControlHostProjectionRestoreState::Archive {
                source_records,
                archive_bytes,
                archive_sha256,
            } => {
                *source_records > 0
                    && *source_records <= limits.max_files
                    && *archive_bytes > 0
                    && *archive_bytes <= limits.max_payload_bytes
                    && valid_sha256(archive_sha256)
            }
        };
        if self.schema != RESTORE_RESULT_SCHEMA
            || !valid_sha256(&self.owner_manifest_digest)
            || !valid_sha256(&self.inventory_digest)
            || !payload_valid
            || !valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_invalid(
                "The Control Host projection restore result is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn validate_for_snapshot(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlHostProjectionSnapshot,
    ) -> UseResult<()> {
        self.validate(registry)?;
        snapshot.validate(registry, &snapshot.manifest.binding)?;
        let payload_matches = match (&self.payload, &snapshot.manifest.payload) {
            (ControlHostProjectionRestoreState::Absent, ControlHostProjectionState::Absent) => true,
            (
                ControlHostProjectionRestoreState::Archive {
                    source_records,
                    archive_bytes,
                    archive_sha256,
                },
                ControlHostProjectionState::Archive {
                    archive_bytes: expected_bytes,
                    archive_sha256: expected_sha256,
                },
            ) => {
                *source_records == snapshot.manifest.entries.len() as u64
                    && archive_bytes == expected_bytes
                    && archive_sha256 == expected_sha256
            }
            _ => false,
        };
        if self.binding != snapshot.manifest.binding
            || self.owner_manifest_digest != snapshot.manifest.descriptor_digest
            || self.inventory_digest != snapshot.manifest.inventory_digest
            || !payload_matches
        {
            return Err(restore_invalid(
                "The Control Host projection restore result differs from its exact owner snapshot.",
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
            payload: &'a ControlHostProjectionRestoreState,
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            binding: &self.binding,
            owner_manifest_digest: &self.owner_manifest_digest,
            inventory_digest: &self.inventory_digest,
            payload: &self.payload,
        })
        .map_err(|error| restore_invalid(format!("Failed to encode restore result: {error}")))?;
        if bytes.is_empty() || bytes.len() > MAX_RESTORE_RESULT_BYTES {
            return Err(restore_invalid(
                "The Control Host projection restore result exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(RESTORE_RESULT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlHostProjectionRestore {
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlHostProjectionSnapshot,
    records: Vec<crate::cognitive_package::HostProjectionSnapshotRecord>,
    state_root: PathBuf,
    staging_directory: PathBuf,
    candidate: Option<PathBuf>,
    canonical: Option<filesystem::CanonicalHostProjection>,
    activation_bytes: Vec<u8>,
}

impl VerifiedControlHostProjectionSnapshot {
    pub(in crate::control_store) async fn stage_clean_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
    ) -> UseResult<StagedControlHostProjectionRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        filesystem::validate_staging_location(&state_root, &staging_directory)?;
        let _maintenance = StateMaintenanceLock::new(&state_root)
            .acquire_shared()
            .await
            .map_err(wrap_restore_error)?;
        filesystem::ensure_owned_directory(&state_root, &staging_directory).await?;
        filesystem::validate_staging_entries(&staging_directory).await?;
        let activation_bytes = activation_bytes(&self.snapshot)?;
        let activation_started =
            filesystem::recover_activation_marker(&staging_directory, &activation_bytes).await?;
        let limits = host_projection_contract(&self.registry)?;

        let (candidate, canonical) = match (
            &self.snapshot.manifest.payload,
            self.archive_path.as_deref(),
        ) {
            (ControlHostProjectionState::Absent, None) => {
                filesystem::require_absent_staging(&staging_directory).await?;
                (None, None)
            }
            (ControlHostProjectionState::Archive { .. }, Some(source)) => {
                let staged_archive =
                    filesystem::stage_archive(source, &staging_directory, &self.snapshot).await?;
                let canonical = filesystem::prepare_candidate(
                    &staged_archive,
                    &staging_directory,
                    &self.snapshot,
                    &self.records,
                    limits,
                    !activation_started,
                )
                .await?;
                (
                    Some(filesystem::candidate_path(&staging_directory)),
                    Some(canonical),
                )
            }
            _ => {
                return Err(restore_invalid(
                    "The verified Host projection omitted or added archive bytes.",
                ))
            }
        };
        filesystem::validate_staging_entries(&staging_directory).await?;
        Ok(StagedControlHostProjectionRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            records: self.records.clone(),
            state_root,
            staging_directory,
            candidate,
            canonical,
            activation_bytes,
        })
    }
}

impl StagedControlHostProjectionRestore {
    pub(in crate::control_store) fn candidate_path(&self) -> Option<&Path> {
        self.candidate.as_deref()
    }

    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlHostProjectionRestoreResult> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Control Host projection activation requires the exact target's exclusive maintenance guard.",
            ));
        }
        filesystem::validate_staging_entries(&self.staging_directory).await?;
        let limits = host_projection_contract(&self.registry)?;
        match (&self.snapshot.manifest.payload, &self.canonical) {
            (ControlHostProjectionState::Absent, None) => {
                filesystem::require_absent_staging(&self.staging_directory).await?;
                if !matches!(
                    filesystem::inspect_live_root(&self.state_root).await?,
                    filesystem::LiveHostRoot::Absent
                ) {
                    return Err(restore_target_not_empty());
                }
            }
            (ControlHostProjectionState::Archive { .. }, Some(canonical)) => {
                let staged_archive =
                    filesystem::staged_archive(&self.staging_directory, &self.snapshot).await?;
                super::archive::verify_archive(&self.snapshot, Some(&staged_archive))
                    .await
                    .map_err(wrap_restore_error)?;
                let started = filesystem::recover_activation_marker(
                    &self.staging_directory,
                    &self.activation_bytes,
                )
                .await?;
                filesystem::activate_candidate(
                    &self.state_root,
                    &self.staging_directory,
                    &self.snapshot,
                    &self.records,
                    canonical,
                    limits,
                    started,
                    &self.activation_bytes,
                )
                .await?;
            }
            _ => {
                return Err(restore_invalid(
                    "The staged Host projection differs from its snapshot payload state.",
                ))
            }
        }
        ControlHostProjectionRestoreResult::new(&self.registry, &self.snapshot)
    }
}

fn activation_bytes(snapshot: &ControlHostProjectionSnapshot) -> UseResult<Vec<u8>> {
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
    .map_err(|error| restore_invalid(format!("Failed to encode activation marker: {error}")))?;
    if bytes.is_empty() || bytes.len() > MAX_ACTIVATION_BYTES {
        return Err(restore_invalid(
            "The Host projection activation marker exceeds its byte bound.",
        ));
    }
    Ok(bytes)
}

fn wrap_restore_error(error: UseError) -> UseError {
    restore_invalid(format!(
        "Host projection restore verification failed: {}",
        error.message
    ))
}

pub(super) fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.host_projection_restore_target_not_empty",
        "The clean-target Host projection restore refuses to merge or replace an existing owner root.",
    )
}

pub(super) fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.control_store.host_projection_restore_invalid", message)
}
