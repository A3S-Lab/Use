use std::path::{Path, PathBuf};

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{
    archive, observation_contract, ControlObservationPayloadEntry,
    ControlObservationPayloadSnapshot, ControlObservationPayloadState, ControlPayloadOwnerRegistry,
    ControlPayloadSnapshotBinding, VerifiedControlObservationPayloadSnapshot,
};
use crate::control_store::model::valid_sha256;
use crate::control_store::payload_owner::canonical_json;

mod filesystem;

const RESTORE_RESULT_SCHEMA: &str = "a3s.use.control-observation-payload-restore-result.v1";
const RESTORE_RESULT_DOMAIN: &[u8] = b"a3s.use.control-observation-payload-restore-result.v1\0";
const MAX_RESTORE_RESULT_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "payloadState",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub(in crate::control_store) enum ControlObservationPayloadRestoreState {
    Absent,
    Archive {
        terminal_records: u64,
        archive_bytes: u64,
        archive_sha256: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(in crate::control_store) struct ControlObservationPayloadRestoreResult {
    schema: String,
    binding: ControlPayloadSnapshotBinding,
    owner_manifest_digest: String,
    inventory_digest: String,
    pub(in crate::control_store) payload: ControlObservationPayloadRestoreState,
    descriptor_digest: String,
}

impl ControlObservationPayloadRestoreResult {
    fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlObservationPayloadSnapshot,
    ) -> UseResult<Self> {
        let payload = match &snapshot.manifest.payload {
            ControlObservationPayloadState::Absent => ControlObservationPayloadRestoreState::Absent,
            ControlObservationPayloadState::Archive {
                archive_bytes,
                archive_sha256,
            } => ControlObservationPayloadRestoreState::Archive {
                terminal_records: u64::try_from(snapshot.manifest.entries.len()).map_err(|_| {
                    restore_invalid("The restored terminal record count overflowed.")
                })?,
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
        let limits = observation_contract(registry)?;
        self.binding.validate(registry)?;
        let payload_valid = match &self.payload {
            ControlObservationPayloadRestoreState::Absent => true,
            ControlObservationPayloadRestoreState::Archive {
                terminal_records,
                archive_bytes,
                archive_sha256,
            } => {
                *terminal_records > 0
                    && *terminal_records <= limits.max_files
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
                "The Control observation restore result is invalid or was rebound.",
            ));
        }
        Ok(())
    }

    pub(in crate::control_store) fn validate_for_snapshot(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlObservationPayloadSnapshot,
    ) -> UseResult<()> {
        self.validate(registry)?;
        snapshot.validate(registry, &snapshot.manifest.binding)?;
        let payload_matches = match (&self.payload, &snapshot.manifest.payload) {
            (
                ControlObservationPayloadRestoreState::Absent,
                ControlObservationPayloadState::Absent,
            ) => true,
            (
                ControlObservationPayloadRestoreState::Archive {
                    terminal_records,
                    archive_bytes,
                    archive_sha256,
                },
                ControlObservationPayloadState::Archive {
                    archive_bytes: expected_bytes,
                    archive_sha256: expected_sha256,
                },
            ) => {
                *terminal_records == snapshot.manifest.entries.len() as u64
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
                "The Control observation restore result differs from its exact owner snapshot.",
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
            payload: &'a ControlObservationPayloadRestoreState,
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
                "The Control observation restore result exceeds its byte bound.",
            ));
        }
        let mut digest = Sha256::new();
        digest.update(RESTORE_RESULT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlObservationPayloadRestore {
    registry: ControlPayloadOwnerRegistry,
    snapshot: ControlObservationPayloadSnapshot,
    state_root: PathBuf,
    staging_directory: PathBuf,
    candidate: Option<PathBuf>,
}

impl VerifiedControlObservationPayloadSnapshot {
    pub(in crate::control_store) async fn stage_clean_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
    ) -> UseResult<StagedControlObservationPayloadRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        filesystem::validate_staging_location(&state_root, &staging_directory)?;
        let _maintenance = StateMaintenanceLock::new(&state_root)
            .acquire_shared()
            .await
            .map_err(wrap_restore_error)?;
        self.stage_clean_restore_inner(state_root, staging_directory)
            .await
    }

    pub(in crate::control_store) async fn stage_clean_restore_under_exclusive(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlObservationPayloadRestore> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        filesystem::validate_staging_location(&state_root, &staging_directory)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Control observation staging requires the exact target's exclusive maintenance guard.",
            ));
        }
        self.stage_clean_restore_inner(state_root, staging_directory)
            .await
    }

    async fn stage_clean_restore_inner(
        &self,
        state_root: PathBuf,
        staging_directory: PathBuf,
    ) -> UseResult<StagedControlObservationPayloadRestore> {
        filesystem::ensure_owned_directory(&state_root, &staging_directory).await?;
        filesystem::validate_staging_entries(&staging_directory, &self.snapshot).await?;
        let candidate = match (
            &self.snapshot.manifest.payload,
            self.archive_path.as_deref(),
        ) {
            (ControlObservationPayloadState::Absent, None) => {
                filesystem::require_empty_staging(&staging_directory).await?;
                None
            }
            (ControlObservationPayloadState::Archive { .. }, Some(source)) => {
                filesystem::stage_archive(source, &staging_directory, &self.snapshot).await?;
                Some(filesystem::candidate_path(&staging_directory))
            }
            _ => {
                return Err(restore_invalid(
                    "The verified observation snapshot omitted or added archive bytes.",
                ))
            }
        };
        filesystem::validate_staging_entries(&staging_directory, &self.snapshot).await?;
        Ok(StagedControlObservationPayloadRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            state_root,
            staging_directory,
            candidate,
        })
    }
}

impl StagedControlObservationPayloadRestore {
    pub(in crate::control_store) fn candidate_path(&self) -> Option<&Path> {
        self.candidate.as_deref()
    }

    pub(in crate::control_store) async fn activate(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<ControlObservationPayloadRestoreResult> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Control observation activation requires the exact target's exclusive maintenance guard.",
            ));
        }
        filesystem::validate_staging_entries(&self.staging_directory, &self.snapshot).await?;
        let limits = observation_contract(&self.registry)?;
        match &self.snapshot.manifest.payload {
            ControlObservationPayloadState::Absent => {
                filesystem::require_empty_staging(&self.staging_directory).await?;
                let live = inspect_live(&self.state_root, &self.snapshot, limits).await?;
                require_clean_target(&live)?;
            }
            ControlObservationPayloadState::Archive { .. } => {
                let staged =
                    filesystem::staged_archive_state(&self.staging_directory, &self.snapshot)
                        .await?;
                archive::verify_archive(&self.snapshot, Some(staged.path()))
                    .await
                    .map_err(wrap_restore_error)?;
                let live = inspect_live(&self.state_root, &self.snapshot, limits).await?;
                match staged {
                    filesystem::StagedArchiveState::Ready(path) => {
                        require_clean_target(&live)?;
                        let activating =
                            filesystem::begin_activation(path, &self.staging_directory).await?;
                        filesystem::activate_archive(
                            &activating,
                            &self.staging_directory,
                            &self.state_root,
                            &self.snapshot,
                            &live.terminal,
                        )
                        .await?;
                    }
                    filesystem::StagedArchiveState::Activating(path) => {
                        require_exact_subset(&live, &self.snapshot.manifest.entries)?;
                        filesystem::activate_archive(
                            &path,
                            &self.staging_directory,
                            &self.state_root,
                            &self.snapshot,
                            &live.terminal,
                        )
                        .await?;
                    }
                }
                let final_live = inspect_live(&self.state_root, &self.snapshot, limits).await?;
                if final_live.active_count != 0
                    || final_live.terminal != self.snapshot.manifest.entries
                {
                    return Err(restore_invalid(
                        "The activated observation payload differs from its exact snapshot.",
                    ));
                }
            }
        }
        ControlObservationPayloadRestoreResult::new(&self.registry, &self.snapshot)
    }
}

async fn inspect_live(
    state_root: &Path,
    snapshot: &ControlObservationPayloadSnapshot,
    limits: super::ControlPayloadOwnerLimits,
) -> UseResult<archive::LiveObservationInventory> {
    archive::inspect_live_inventory(state_root, &snapshot.manifest.binding.installation, limits)
        .await
        .map_err(wrap_restore_error)
}

fn require_clean_target(live: &archive::LiveObservationInventory) -> UseResult<()> {
    if live.active_count != 0 || !live.terminal.is_empty() {
        return Err(restore_target_not_empty());
    }
    Ok(())
}

fn require_exact_subset(
    live: &archive::LiveObservationInventory,
    expected: &[ControlObservationPayloadEntry],
) -> UseResult<()> {
    if live.active_count != 0
        || live.terminal.iter().any(|entry| {
            expected
                .binary_search_by(|candidate| candidate.path.cmp(&entry.path))
                .ok()
                .is_none_or(|index| expected[index] != *entry)
        })
    {
        return Err(restore_target_not_empty());
    }
    Ok(())
}

fn wrap_restore_error(error: UseError) -> UseError {
    restore_invalid(format!(
        "Observation restore verification failed: {}",
        error.message
    ))
}

pub(super) fn restore_target_not_empty() -> UseError {
    UseError::new(
        "use.control_store.observation_payload_restore_target_not_empty",
        "The clean-target observation restore refuses to merge existing planning or diagnostic records.",
    )
}

pub(super) fn restore_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.observation_payload_restore_invalid",
        message,
    )
}
