use std::io;
use std::path::PathBuf;

use a3s_use_core::{InstallationId, UseError, UseResult};
use a3s_use_extension::{StateMaintenanceGuard, StateMaintenanceLock};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::host_projection::StagedControlHostProjectionRestore;
use super::super::knowledge::StagedControlKnowledgePayloadRestore;
use super::super::observations::StagedControlObservationPayloadRestore;
use super::super::restore_coordinator::StagedControlRestoreCoordinatorRestore;
use super::super::runtime_plans::StagedControlRuntimePlanPayloadRestore;
use super::super::{ControlPayloadOwnerId, ControlPayloadSnapshotBinding};
use super::control_restore::{self, StagedControlStoreRestore};
use super::coordinator::VerifiedControlInstallationSnapshot;
use super::restore_activation::ControlInstallationRestoreResult;
use super::restore_filesystem::{
    self, CONTROL_DIRECTORY, HOST_PROJECTION_DIRECTORY, KNOWLEDGE_DIRECTORY,
    OBSERVATIONS_DIRECTORY, RESTORE_COORDINATOR_DIRECTORY, RUNTIME_PLANS_DIRECTORY,
};
use super::{canonical_json, ControlInstallationSnapshotManifest, ControlPayloadOwnerRegistry};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

const RESTORE_ATTEMPT_SCHEMA: &str = "a3s.use.control-installation-restore-attempt.v2";
const RESTORE_ATTEMPT_DOMAIN: &[u8] = b"a3s.use.control-installation-restore-attempt.v2\0";
pub(super) const MAX_RESTORE_ATTEMPT_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum RestoreComponent {
    ControlStore,
    RuntimePlans,
    HostProjection,
    Knowledge,
    Observations,
    RestoreCoordinator,
}

impl RestoreComponent {
    pub(super) const ALL: [Self; 6] = [
        Self::ControlStore,
        Self::RuntimePlans,
        Self::HostProjection,
        Self::Knowledge,
        Self::Observations,
        Self::RestoreCoordinator,
    ];

    pub(super) const fn label(self) -> &'static str {
        match self {
            Self::ControlStore => "control-store",
            Self::RuntimePlans => "runtime-plans",
            Self::HostProjection => "host-projection",
            Self::Knowledge => "knowledge",
            Self::Observations => "observations",
            Self::RestoreCoordinator => "restore-coordinator",
        }
    }

    pub(super) const fn staging_directory_name(self) -> &'static str {
        match self {
            Self::ControlStore => CONTROL_DIRECTORY,
            Self::RuntimePlans => RUNTIME_PLANS_DIRECTORY,
            Self::HostProjection => HOST_PROJECTION_DIRECTORY,
            Self::Knowledge => KNOWLEDGE_DIRECTORY,
            Self::Observations => OBSERVATIONS_DIRECTORY,
            Self::RestoreCoordinator => RESTORE_COORDINATOR_DIRECTORY,
        }
    }

    pub(super) const fn payload_owner(self) -> Option<ControlPayloadOwnerId> {
        match self {
            Self::ControlStore => None,
            Self::RuntimePlans => Some(ControlPayloadOwnerId::RuntimePlanPayload),
            Self::HostProjection => Some(ControlPayloadOwnerId::HostProtocolProjection),
            Self::Knowledge => Some(ControlPayloadOwnerId::KnowledgePayload),
            Self::Observations => Some(ControlPayloadOwnerId::PlanningAndDiagnosticObservations),
            Self::RestoreCoordinator => Some(ControlPayloadOwnerId::RestoreCoordinator),
        }
    }

    pub(super) fn index(self) -> usize {
        Self::ALL
            .into_iter()
            .position(|candidate| candidate == self)
            .unwrap_or(Self::ALL.len())
    }

    pub(super) fn for_payload_owner(owner: ControlPayloadOwnerId) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|component| component.payload_owner() == Some(owner))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct KnowledgePolicyEvidence {
    max_scope_expanded_bytes: u64,
    max_scope_projections: u64,
    max_surface_generations: u64,
    max_scope_tombstones: u64,
}

impl KnowledgePolicyEvidence {
    fn new(policy: OkfKnowledgeStoragePolicy) -> UseResult<Self> {
        Ok(Self {
            max_scope_expanded_bytes: policy.max_scope_expanded_bytes(),
            max_scope_projections: u64::try_from(policy.max_scope_projections()).map_err(|_| {
                restore_staging_invalid("The Knowledge projection policy overflowed.")
            })?,
            max_surface_generations: u64::try_from(policy.max_surface_generations()).map_err(
                |_| restore_staging_invalid("The Knowledge generation policy overflowed."),
            )?,
            max_scope_tombstones: u64::try_from(policy.max_scope_tombstones()).map_err(|_| {
                restore_staging_invalid("The Knowledge tombstone policy overflowed.")
            })?,
        })
    }

    fn validate(&self) -> UseResult<()> {
        OkfKnowledgeStoragePolicy::new(
            self.max_scope_expanded_bytes,
            usize::try_from(self.max_scope_projections).map_err(|_| {
                restore_staging_invalid("The Knowledge projection policy overflowed.")
            })?,
            usize::try_from(self.max_surface_generations).map_err(|_| {
                restore_staging_invalid("The Knowledge generation policy overflowed.")
            })?,
            usize::try_from(self.max_scope_tombstones).map_err(|_| {
                restore_staging_invalid("The Knowledge tombstone policy overflowed.")
            })?,
        )
        .map(|_| ())
        .map_err(|error| {
            restore_staging_invalid(format!(
                "The Knowledge policy evidence is invalid: {}",
                error.message
            ))
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(super) struct ControlInstallationRestoreAttempt {
    schema: String,
    snapshot_created_at_ms: u64,
    snapshot_descriptor_digest: String,
    binding: ControlPayloadSnapshotBinding,
    owner_registry_digest: String,
    knowledge_policy: KnowledgePolicyEvidence,
    components: Vec<RestoreComponent>,
    descriptor_digest: String,
}

impl ControlInstallationRestoreAttempt {
    pub(super) fn new(
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlInstallationSnapshotManifest,
        policy: OkfKnowledgeStoragePolicy,
    ) -> UseResult<Self> {
        snapshot.validate(registry).map_err(|error| {
            restore_staging_invalid(format!(
                "The complete restore snapshot is invalid: {}",
                error.message
            ))
        })?;
        let mut attempt = Self {
            schema: RESTORE_ATTEMPT_SCHEMA.to_owned(),
            snapshot_created_at_ms: snapshot.created_at_ms,
            snapshot_descriptor_digest: snapshot.descriptor_digest.clone(),
            binding: snapshot.snapshot_set.binding.clone(),
            owner_registry_digest: registry.descriptor_digest().to_owned(),
            knowledge_policy: KnowledgePolicyEvidence::new(policy)?,
            components: RestoreComponent::ALL.to_vec(),
            descriptor_digest: String::new(),
        };
        attempt.descriptor_digest = attempt.expected_digest()?;
        attempt.validate(registry, snapshot)?;
        Ok(attempt)
    }

    fn validate(
        &self,
        registry: &ControlPayloadOwnerRegistry,
        snapshot: &ControlInstallationSnapshotManifest,
    ) -> UseResult<()> {
        self.validate_descriptor()?;
        self.binding.validate(registry).map_err(|error| {
            restore_staging_invalid(format!(
                "The complete restore binding is invalid: {}",
                error.message
            ))
        })?;
        if self.snapshot_created_at_ms != snapshot.created_at_ms
            || self.snapshot_descriptor_digest != snapshot.descriptor_digest
            || self.binding != snapshot.snapshot_set.binding
            || self.owner_registry_digest != registry.descriptor_digest()
        {
            return Err(restore_staging_invalid(
                "The complete restore attempt is incomplete, noncanonical, or was rebound.",
            ));
        }
        Ok(())
    }

    fn validate_descriptor(&self) -> UseResult<()> {
        self.binding.validate_descriptor().map_err(|error| {
            restore_staging_invalid(format!(
                "The complete restore binding descriptor is invalid: {}",
                error.message
            ))
        })?;
        self.knowledge_policy.validate()?;
        if self.schema != RESTORE_ATTEMPT_SCHEMA
            || self.snapshot_created_at_ms == 0
            || !crate::control_store::model::valid_sha256(&self.snapshot_descriptor_digest)
            || !crate::control_store::model::valid_sha256(&self.owner_registry_digest)
            || self.owner_registry_digest != self.binding.owner_registry_digest
            || self.components != RestoreComponent::ALL
            || !crate::control_store::model::valid_sha256(&self.descriptor_digest)
            || self.expected_digest()? != self.descriptor_digest
        {
            return Err(restore_staging_invalid(
                "The complete restore attempt is incomplete, noncanonical, or was rebound.",
            ));
        }
        Ok(())
    }

    fn expected_digest(&self) -> UseResult<String> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Descriptor<'a> {
            schema: &'a str,
            snapshot_created_at_ms: u64,
            snapshot_descriptor_digest: &'a str,
            binding: &'a ControlPayloadSnapshotBinding,
            owner_registry_digest: &'a str,
            knowledge_policy: &'a KnowledgePolicyEvidence,
            components: &'a [RestoreComponent],
        }
        let bytes = canonical_json(&Descriptor {
            schema: &self.schema,
            snapshot_created_at_ms: self.snapshot_created_at_ms,
            snapshot_descriptor_digest: &self.snapshot_descriptor_digest,
            binding: &self.binding,
            owner_registry_digest: &self.owner_registry_digest,
            knowledge_policy: &self.knowledge_policy,
            components: &self.components,
        })
        .map_err(|error| {
            restore_staging_invalid(format!(
                "Failed to encode the complete restore attempt: {error}"
            ))
        })?;
        let mut digest = Sha256::new();
        digest.update(RESTORE_ATTEMPT_DOMAIN);
        digest.update(bytes);
        Ok(format!("sha256:{:x}", digest.finalize()))
    }

    pub(super) fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate_descriptor()?;
        let bytes = canonical_json(self).map_err(|error| {
            restore_staging_invalid(format!(
                "Failed to encode the complete restore descriptor: {error}"
            ))
        })?;
        if bytes.is_empty() || bytes.len() > MAX_RESTORE_ATTEMPT_BYTES {
            return Err(restore_staging_invalid(
                "The complete restore descriptor exceeds its byte bound.",
            ));
        }
        Ok(bytes)
    }

    pub(super) fn descriptor_digest(&self) -> &str {
        &self.descriptor_digest
    }

    pub(super) fn installation(&self) -> &InstallationId {
        &self.binding.installation
    }

    pub(super) fn decode_canonical(bytes: &[u8]) -> UseResult<Self> {
        if bytes.is_empty() || bytes.len() > MAX_RESTORE_ATTEMPT_BYTES {
            return Err(restore_staging_invalid(
                "The complete restore descriptor exceeds its byte bound.",
            ));
        }
        let attempt: Self = serde_json::from_slice(bytes).map_err(|_| {
            restore_staging_invalid("The complete restore descriptor is invalid JSON.")
        })?;
        attempt.validate_descriptor()?;
        if attempt.canonical_bytes()? != bytes {
            return Err(restore_staging_invalid(
                "The complete restore descriptor is not canonically encoded.",
            ));
        }
        Ok(attempt)
    }
}

#[derive(Debug)]
pub(in crate::control_store) struct StagedControlInstallationRestore {
    pub(super) state_root: PathBuf,
    pub(super) staging_directory: PathBuf,
    pub(super) attempt_bytes: Vec<u8>,
    pub(super) attempt_digest: String,
    pub(super) state: ControlInstallationRestoreState,
    pub(super) maintenance: StateMaintenanceGuard,
}

#[derive(Debug)]
pub(super) enum ControlInstallationRestoreState {
    Prepared(Box<PreparedControlInstallationRestore>),
    Retired(ControlInstallationRestoreResult),
}

#[derive(Debug)]
pub(super) struct PreparedControlInstallationRestore {
    pub(super) control: StagedControlStoreRestore,
    pub(super) host_projection: StagedControlHostProjectionRestore,
    pub(super) runtime_plans: StagedControlRuntimePlanPayloadRestore,
    pub(super) knowledge: StagedControlKnowledgePayloadRestore,
    pub(super) observations: StagedControlObservationPayloadRestore,
    pub(super) restore_coordinator: StagedControlRestoreCoordinatorRestore,
}

impl VerifiedControlInstallationSnapshot {
    pub(in crate::control_store) async fn stage_clean_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        knowledge_policy: OkfKnowledgeStoragePolicy,
    ) -> UseResult<StagedControlInstallationRestore> {
        self.knowledge
            .validate_restore_policy(knowledge_policy)
            .map_err(|error| wrap_owner_error("Knowledge policy", error))?;
        let attempt = ControlInstallationRestoreAttempt::new(
            &self.registry,
            &self.manifest,
            knowledge_policy,
        )?;
        let attempt_bytes = attempt.canonical_bytes()?;
        let state_root = target_state_root.into();
        let maintenance = StateMaintenanceLock::new(&state_root)
            .acquire_exclusive()
            .await
            .map_err(|error| {
                restore_staging_invalid(format!(
                    "Failed to acquire the complete restore fence: {}",
                    error.message
                ))
            })?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_staging_invalid(
                "The complete restore did not retain its exact target fence.",
            ));
        }
        let staging_directory = restore_filesystem::prepare_attempt(&state_root, &attempt_bytes)
            .await
            .map_err(wrap_stage_error)?;
        let control = control_restore::stage(
            &self.registry,
            &self.manifest.descriptor_digest,
            &self.manifest.snapshot_set.binding,
            &self.control_export,
            &state_root,
            &restore_filesystem::component_directory(&staging_directory, CONTROL_DIRECTORY),
            &maintenance,
        )
        .await
        .map_err(wrap_stage_error)?;
        let runtime_plans = self
            .runtime_plans
            .stage_clean_restore_under_exclusive(
                state_root.clone(),
                restore_filesystem::component_directory(
                    &staging_directory,
                    RUNTIME_PLANS_DIRECTORY,
                ),
                &maintenance,
            )
            .await
            .map_err(|error| wrap_owner_error("Runtime plan payload", error))?;
        let host_projection = self
            .host_projection
            .stage_clean_restore_under_exclusive(
                state_root.clone(),
                restore_filesystem::component_directory(
                    &staging_directory,
                    HOST_PROJECTION_DIRECTORY,
                ),
                &maintenance,
            )
            .await
            .map_err(|error| wrap_owner_error("Host projection", error))?;
        let knowledge = self
            .knowledge
            .stage_clean_restore_under_exclusive(
                state_root.clone(),
                restore_filesystem::component_directory(&staging_directory, KNOWLEDGE_DIRECTORY),
                knowledge_policy,
                &maintenance,
            )
            .await
            .map_err(|error| wrap_owner_error("Knowledge", error))?;
        let observations = self
            .observations
            .stage_clean_restore_under_exclusive(
                state_root.clone(),
                restore_filesystem::component_directory(&staging_directory, OBSERVATIONS_DIRECTORY),
                &maintenance,
            )
            .await
            .map_err(|error| wrap_owner_error("observations", error))?;
        let restore_coordinator = self
            .restore_coordinator
            .stage_restore_under_exclusive(
                state_root.clone(),
                restore_filesystem::component_directory(
                    &staging_directory,
                    RESTORE_COORDINATOR_DIRECTORY,
                ),
                &maintenance,
            )
            .await
            .map_err(|error| wrap_owner_error("Restore Coordinator", error))?;
        restore_filesystem::validate_complete_attempt(
            &state_root,
            &staging_directory,
            &attempt_bytes,
        )
        .await
        .map_err(wrap_stage_error)?;
        Ok(StagedControlInstallationRestore {
            state_root,
            staging_directory,
            attempt_bytes,
            attempt_digest: attempt.descriptor_digest,
            state: ControlInstallationRestoreState::Prepared(Box::new(
                PreparedControlInstallationRestore {
                    control,
                    runtime_plans,
                    host_projection,
                    knowledge,
                    observations,
                    restore_coordinator,
                },
            )),
            maintenance,
        })
    }
}

pub(super) fn wrap_owner_error(owner: &str, error: UseError) -> UseError {
    restore_staging_invalid(format!(
        "The complete restore {owner} candidate failed verification: {}",
        error.message
    ))
}

fn wrap_stage_error(error: UseError) -> UseError {
    if error.code == "use.control_store.complete_restore_staging_invalid" {
        error
    } else {
        restore_staging_invalid(format!(
            "The complete restore staging boundary rejected its state: {}",
            error.message
        ))
    }
}

pub(super) fn restore_staging_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.complete_restore_staging_invalid",
        message,
    )
}

pub(super) fn restore_staging_io(action: &str, error: io::Error) -> UseError {
    restore_staging_invalid(format!("Failed to {action}: {error}"))
}

pub(super) fn wrap_activation_error(error: UseError) -> UseError {
    if error.code == "use.control_store.complete_restore_activation_invalid" {
        error
    } else {
        restore_activation_invalid(format!(
            "The complete restore activation boundary rejected its state: {}",
            error.message
        ))
    }
}

pub(super) fn restore_activation_invalid(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.control_store.complete_restore_activation_invalid",
        message,
    )
}

pub(super) fn restore_activation_io(action: &str, error: io::Error) -> UseError {
    restore_activation_invalid(format!("Failed to {action}: {error}"))
}
