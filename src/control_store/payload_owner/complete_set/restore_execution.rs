//! Ordered execution and replay for a staged complete restore.

#[cfg(test)]
use std::path::{Path, PathBuf};

use a3s_use_core::UseResult;

#[cfg(test)]
use super::control_restore::StagedControlStoreRestore;
use super::control_restore_result::ControlStoreRestoreResult;
#[cfg(test)]
use super::restore::restore_activation_invalid;
use super::restore::{
    wrap_activation_error, wrap_owner_error, RestoreComponent, StagedControlInstallationRestore,
};
use super::restore_activation::{self, ControlInstallationRestoreResult};

impl StagedControlInstallationRestore {
    pub(in crate::control_store) async fn activate(
        &self,
    ) -> UseResult<ControlInstallationRestoreResult> {
        if restore_activation::journal_exists(&self.staging_directory)
            .await
            .map_err(wrap_activation_error)?
        {
            let current = restore_activation::load(
                &self.state_root,
                &self.staging_directory,
                &self.attempt_digest,
            )
            .await
            .map_err(wrap_activation_error)?;
            if current.is_complete()
                && !restore_activation::marker_exists(&self.state_root)
                    .await
                    .map_err(wrap_activation_error)?
            {
                return current
                    .completed_result(&self.attempt_digest)
                    .map_err(wrap_activation_error);
            }
        }

        for component in RestoreComponent::ALL {
            self.activate_component(component, true).await?;
        }
        restore_activation::complete(
            &self.state_root,
            &self.staging_directory,
            &self.attempt_digest,
        )
        .await
        .map_err(wrap_activation_error)
    }

    pub(in crate::control_store) async fn activate_control(
        &self,
    ) -> UseResult<ControlStoreRestoreResult> {
        self.activate_control_component(true).await
    }

    async fn activate_control_component(
        &self,
        checkpoint: bool,
    ) -> UseResult<ControlStoreRestoreResult> {
        if restore_activation::journal_exists(&self.staging_directory)
            .await
            .map_err(wrap_activation_error)?
        {
            self.control
                .preflight(&self.maintenance)
                .await
                .map_err(wrap_activation_error)?;
        } else {
            self.preflight_clean().await?;
        }
        restore_activation::begin(
            &self.state_root,
            &self.staging_directory,
            &self.attempt_bytes,
            &self.attempt_digest,
            self.control.candidate_path(),
        )
        .await
        .map_err(wrap_activation_error)?;
        let result = self
            .control
            .activate(&self.maintenance)
            .await
            .map_err(wrap_activation_error)?;
        restore_activation::maybe_test_crash("control-store-effect");
        restore_activation::validate_result_registry(&self.control.registry, &result)
            .map_err(wrap_activation_error)?;
        if checkpoint {
            restore_activation::checkpoint_control(
                &self.state_root,
                &self.staging_directory,
                &self.attempt_digest,
                &result,
            )
            .await
            .map_err(wrap_activation_error)?;
        }
        Ok(result)
    }

    async fn preflight_clean(&self) -> UseResult<()> {
        self.control
            .preflight(&self.maintenance)
            .await
            .map_err(wrap_activation_error)?;
        self.host_projection
            .preflight_clean(&self.maintenance)
            .await
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("Host projection preflight", error))
            })?;
        self.knowledge
            .preflight_clean(&self.maintenance)
            .await
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("Knowledge preflight", error))
            })?;
        self.observations
            .preflight_clean(&self.maintenance)
            .await
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("observations preflight", error))
            })?;
        self.restore_coordinator
            .preflight_clean(&self.maintenance)
            .await
            .map_err(|error| {
                wrap_activation_error(wrap_owner_error("Restore Coordinator preflight", error))
            })
    }

    async fn activate_component(
        &self,
        component: RestoreComponent,
        checkpoint: bool,
    ) -> UseResult<()> {
        match component {
            RestoreComponent::ControlStore => {
                self.activate_control_component(checkpoint).await?;
            }
            RestoreComponent::HostProjection => {
                let result = self
                    .host_projection
                    .activate(&self.maintenance)
                    .await
                    .map_err(|error| {
                        wrap_activation_error(wrap_owner_error("Host projection activation", error))
                    })?;
                restore_activation::maybe_test_crash("host-projection-effect");
                result
                    .validate(&self.control.registry)
                    .map_err(wrap_activation_error)?;
                if checkpoint {
                    self.checkpoint(component, &result).await?;
                }
            }
            RestoreComponent::Knowledge => {
                let result = self
                    .knowledge
                    .activate(&self.maintenance)
                    .await
                    .map_err(|error| {
                        wrap_activation_error(wrap_owner_error("Knowledge activation", error))
                    })?;
                restore_activation::maybe_test_crash("knowledge-effect");
                result
                    .validate(&self.control.registry)
                    .map_err(wrap_activation_error)?;
                if checkpoint {
                    self.checkpoint(component, &result).await?;
                }
            }
            RestoreComponent::Observations => {
                let result = self
                    .observations
                    .activate(&self.maintenance)
                    .await
                    .map_err(|error| {
                        wrap_activation_error(wrap_owner_error("observations activation", error))
                    })?;
                restore_activation::maybe_test_crash("observations-effect");
                result
                    .validate(&self.control.registry)
                    .map_err(wrap_activation_error)?;
                if checkpoint {
                    self.checkpoint(component, &result).await?;
                }
            }
            RestoreComponent::RestoreCoordinator => {
                let activation = restore_activation::load(
                    &self.state_root,
                    &self.staging_directory,
                    &self.attempt_digest,
                )
                .await
                .map_err(wrap_activation_error)?;
                let marker_bytes = activation
                    .active_marker_bytes()
                    .map_err(wrap_activation_error)?;
                let result = self
                    .restore_coordinator
                    .activate_for_complete_restore(
                        &self.maintenance,
                        &self.attempt_digest,
                        &marker_bytes,
                    )
                    .await
                    .map_err(|error| {
                        wrap_activation_error(wrap_owner_error(
                            "Restore Coordinator activation",
                            error,
                        ))
                    })?;
                restore_activation::maybe_test_crash("restore-coordinator-effect");
                if checkpoint {
                    self.checkpoint(component, &result).await?;
                }
            }
        }
        Ok(())
    }

    async fn checkpoint<T: serde::Serialize>(
        &self,
        component: RestoreComponent,
        result: &T,
    ) -> UseResult<()> {
        restore_activation::checkpoint(
            &self.state_root,
            &self.staging_directory,
            &self.attempt_digest,
            component,
            result,
        )
        .await
        .map(|_| ())
        .map_err(wrap_activation_error)
    }

    #[cfg(test)]
    pub(in crate::control_store) async fn activate_next_for_test(&self) -> UseResult<()> {
        self.activate_next_for_test_inner(true).await
    }

    #[cfg(test)]
    pub(in crate::control_store) async fn activate_next_effect_without_checkpoint_for_test(
        &self,
    ) -> UseResult<()> {
        self.activate_next_for_test_inner(false).await
    }

    #[cfg(test)]
    async fn activate_next_for_test_inner(&self, checkpoint: bool) -> UseResult<()> {
        let count = if restore_activation::journal_exists(&self.staging_directory)
            .await
            .map_err(wrap_activation_error)?
        {
            restore_activation::load(
                &self.state_root,
                &self.staging_directory,
                &self.attempt_digest,
            )
            .await
            .map_err(wrap_activation_error)?
            .checkpoint_count()
        } else {
            0
        };
        let component = RestoreComponent::ALL.get(count).copied().ok_or_else(|| {
            restore_activation_invalid("Every complete restore owner is already checkpointed.")
        })?;
        self.activate_component(component, checkpoint).await
    }

    #[cfg(test)]
    pub(in crate::control_store) async fn begin_control_activation_for_test(
        &self,
    ) -> UseResult<()> {
        self.control
            .preflight(&self.maintenance)
            .await
            .map_err(wrap_activation_error)?;
        restore_activation::begin(
            &self.state_root,
            &self.staging_directory,
            &self.attempt_bytes,
            &self.attempt_digest,
            self.control.candidate_path(),
        )
        .await
        .map(|_| ())
        .map_err(wrap_activation_error)
    }

    #[cfg(test)]
    pub(in crate::control_store) fn activation_journal_path_for_test(&self) -> PathBuf {
        restore_activation::journal_path(&self.staging_directory)
    }

    #[cfg(test)]
    pub(in crate::control_store) async fn activation_checkpoint_count_for_test(
        &self,
    ) -> UseResult<usize> {
        restore_activation::load(
            &self.state_root,
            &self.staging_directory,
            &self.attempt_digest,
        )
        .await
        .map(|activation| activation.checkpoint_count())
        .map_err(wrap_activation_error)
    }

    #[cfg(test)]
    pub(in crate::control_store) fn control_restore_for_test(&self) -> &StagedControlStoreRestore {
        &self.control
    }

    #[cfg(test)]
    pub(in crate::control_store) fn holds_exclusive_fence(&self, state_root: &Path) -> bool {
        self.maintenance.is_exclusive_for(state_root)
    }

    #[cfg(test)]
    pub(in crate::control_store) fn control_candidate_path(&self) -> &Path {
        self.control.candidate_path()
    }

    #[cfg(test)]
    pub(in crate::control_store) fn host_projection_candidate_path(&self) -> Option<&Path> {
        self.host_projection.candidate_path()
    }

    #[cfg(test)]
    pub(in crate::control_store) fn knowledge_candidate_path(&self) -> Option<&Path> {
        self.knowledge.candidate_path()
    }

    #[cfg(test)]
    pub(in crate::control_store) fn observation_candidate_path(&self) -> Option<&Path> {
        self.observations.candidate_path()
    }

    #[cfg(test)]
    pub(in crate::control_store) fn restore_coordinator_candidate_path(&self) -> Option<&Path> {
        self.restore_coordinator.candidate_path()
    }

    #[cfg(test)]
    pub(in crate::control_store) fn attempt_digest(&self) -> &str {
        &self.attempt_digest
    }

    #[cfg(test)]
    pub(in crate::control_store) fn staging_directory(&self) -> &Path {
        &self.staging_directory
    }
}
