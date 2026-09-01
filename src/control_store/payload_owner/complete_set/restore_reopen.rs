//! Reopens any durable checkpoint prefix of a complete restore.

use std::path::PathBuf;

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceLock;

use super::control_restore_reopen;
use super::coordinator::VerifiedControlInstallationSnapshot;
use super::restore::{
    restore_activation_invalid, wrap_activation_error, wrap_owner_error,
    ControlInstallationRestoreAttempt, ControlInstallationRestoreState,
    PreparedControlInstallationRestore, StagedControlInstallationRestore,
};
use super::restore_activation;
use super::restore_filesystem::{
    self, CONTROL_DIRECTORY, HOST_PROJECTION_DIRECTORY, KNOWLEDGE_DIRECTORY,
    OBSERVATIONS_DIRECTORY, RESTORE_COORDINATOR_DIRECTORY,
};
use super::restore_retirement;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

impl VerifiedControlInstallationSnapshot {
    pub(in crate::control_store) async fn reopen_activation(
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
            .map_err(wrap_activation_error)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_activation_invalid(
                "The reopened complete restore did not retain its exact target fence.",
            ));
        }
        let staging_directory = state_root.join(restore_filesystem::ATTEMPT_DIRECTORY);
        restore_filesystem::validate_attempt_evidence(&staging_directory, &attempt_bytes)
            .await
            .map_err(wrap_activation_error)?;
        if restore_activation::journal_exists(&staging_directory)
            .await
            .map_err(wrap_activation_error)?
        {
            let activation =
                restore_activation::load_journal(&staging_directory, attempt.descriptor_digest())
                    .await
                    .map_err(wrap_activation_error)?
                    .ok_or_else(|| {
                        wrap_activation_error(restore_activation_invalid(
                            "The complete restore journal disappeared while it was reopened.",
                        ))
                    })?;
            if activation.is_complete()
                && !restore_activation::marker_exists(&state_root)
                    .await
                    .map_err(wrap_activation_error)?
            {
                let result = restore_retirement::finish(
                    &state_root,
                    &staging_directory,
                    &attempt_bytes,
                    attempt.descriptor_digest(),
                )
                .await
                .map_err(wrap_activation_error)?;
                return Ok(StagedControlInstallationRestore {
                    state_root,
                    staging_directory,
                    attempt_bytes,
                    attempt_digest: attempt.descriptor_digest().to_owned(),
                    state: ControlInstallationRestoreState::Retired(result),
                    maintenance,
                });
            }
        }
        restore_filesystem::validate_attempt(&staging_directory, &attempt_bytes)
            .await
            .map_err(wrap_activation_error)?;
        let control_directory =
            restore_filesystem::component_directory(&staging_directory, CONTROL_DIRECTORY);
        let control_candidate = control_directory.join("control.sqlite3");
        restore_activation::begin(
            &state_root,
            &staging_directory,
            &attempt_bytes,
            attempt.descriptor_digest(),
            &control_candidate,
        )
        .await
        .map_err(wrap_activation_error)?;
        let control = control_restore_reopen::reopen(
            &self.registry,
            &self.manifest.descriptor_digest,
            &self.manifest.snapshot_set.binding,
            &state_root,
            &control_directory,
            &maintenance,
        )
        .await
        .map_err(wrap_activation_error)?;
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
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("Host projection replay", error))
            })?;
        let knowledge = self
            .knowledge
            .reopen_staged_restore(
                state_root.clone(),
                restore_filesystem::component_directory(&staging_directory, KNOWLEDGE_DIRECTORY),
                knowledge_policy,
                &maintenance,
            )
            .await
            .map_err(|error| wrap_activation_error(wrap_owner_error("Knowledge replay", error)))?;
        let observations = self
            .observations
            .stage_clean_restore_under_exclusive(
                state_root.clone(),
                restore_filesystem::component_directory(&staging_directory, OBSERVATIONS_DIRECTORY),
                &maintenance,
            )
            .await
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("observations replay", error))
            })?;
        let restore_coordinator = self
            .restore_coordinator
            .reopen_staged_restore(
                state_root.clone(),
                restore_filesystem::component_directory(
                    &staging_directory,
                    RESTORE_COORDINATOR_DIRECTORY,
                ),
                &maintenance,
            )
            .await
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("Restore Coordinator replay", error))
            })?;
        restore_filesystem::validate_attempt(&staging_directory, &attempt_bytes)
            .await
            .map_err(wrap_activation_error)?;
        restore_activation::load(&state_root, &staging_directory, attempt.descriptor_digest())
            .await
            .map_err(wrap_activation_error)?;
        Ok(StagedControlInstallationRestore {
            state_root,
            staging_directory,
            attempt_bytes,
            attempt_digest: attempt.descriptor_digest().to_owned(),
            state: ControlInstallationRestoreState::Prepared(Box::new(
                PreparedControlInstallationRestore {
                    control,
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
