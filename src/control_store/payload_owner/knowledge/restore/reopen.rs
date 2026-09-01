//! Exact reopen and clean-target preflight for a staged Knowledge restore.

use std::path::PathBuf;

use a3s_use_core::UseResult;
use a3s_use_extension::StateMaintenanceGuard;

use super::super::{ControlKnowledgePayloadState, VerifiedControlKnowledgePayloadSnapshot};
use super::filesystem::{
    ensure_owned_directory, inspect_live_payload_layout, optional_regular_file,
    validate_staging_entries, LiveKnowledgePayloadLayout,
};
use super::{
    restore_invalid, restore_target_not_empty, validate_inventory,
    StagedControlKnowledgePayloadRestore, CANDIDATE_FILE, PARTIAL_FILE,
};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

impl VerifiedControlKnowledgePayloadSnapshot {
    pub(in crate::control_store) async fn reopen_staged_restore(
        &self,
        target_state_root: impl Into<PathBuf>,
        staging_directory: impl Into<PathBuf>,
        policy: OkfKnowledgeStoragePolicy,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<StagedControlKnowledgePayloadRestore> {
        self.validate_restore_policy(policy)?;
        let state_root = target_state_root.into();
        let staging_directory = staging_directory.into();
        let adapter = self.restore_adapter(&state_root, &staging_directory, policy)?;
        if !maintenance.is_exclusive_for(&state_root) {
            return Err(restore_invalid(
                "Control Knowledge replay requires the exact target's exclusive maintenance guard.",
            ));
        }
        ensure_owned_directory(&state_root, &staging_directory).await?;
        validate_staging_entries(&staging_directory).await?;
        let staged_candidate = staging_directory.join(CANDIDATE_FILE);
        if optional_regular_file(&staging_directory.join(PARTIAL_FILE)).await? {
            return Err(restore_invalid(
                "The reopened Knowledge restore has a partial candidate.",
            ));
        }
        let candidate_exists = optional_regular_file(&staged_candidate).await?;
        let live =
            inspect_live_payload_layout(&adapter, &self.snapshot.manifest.binding.installation)
                .await?;
        let candidate = match (&self.snapshot.manifest.payload, &self.backup, live) {
            (ControlKnowledgePayloadState::Absent, None, LiveKnowledgePayloadLayout::Absent)
                if !candidate_exists =>
            {
                None
            }
            (
                ControlKnowledgePayloadState::Archive { backup, .. },
                Some(_),
                LiveKnowledgePayloadLayout::Absent | LiveKnowledgePayloadLayout::Empty,
            ) if candidate_exists => {
                validate_inventory(
                    &adapter,
                    &staged_candidate,
                    backup,
                    self.bindings(),
                    self.selected(),
                )
                .await?;
                Some(staged_candidate)
            }
            (
                ControlKnowledgePayloadState::Archive { backup, .. },
                Some(_),
                LiveKnowledgePayloadLayout::Database(live),
            ) if !candidate_exists => {
                validate_inventory(&adapter, &live, backup, self.bindings(), self.selected())
                    .await?;
                Some(staged_candidate)
            }
            _ => {
                return Err(restore_invalid(
                    "The reopened Knowledge restore boundary is missing or ambiguous.",
                ))
            }
        };
        Ok(StagedControlKnowledgePayloadRestore {
            registry: self.registry.clone(),
            snapshot: self.snapshot.clone(),
            bindings: self.bindings().to_vec(),
            selected: self.selected().to_vec(),
            state_root,
            adapter,
            staging_directory,
            candidate,
        })
    }
}

impl StagedControlKnowledgePayloadRestore {
    pub(in crate::control_store) async fn preflight_clean(
        &self,
        maintenance: &StateMaintenanceGuard,
    ) -> UseResult<()> {
        self.snapshot
            .validate(&self.registry, &self.snapshot.manifest.binding)?;
        if !maintenance.is_exclusive_for(&self.state_root) {
            return Err(restore_invalid(
                "Control Knowledge preflight requires the exact target's exclusive maintenance guard.",
            ));
        }
        validate_staging_entries(&self.staging_directory).await?;
        let candidate = self.staging_directory.join(CANDIDATE_FILE);
        if optional_regular_file(&self.staging_directory.join(PARTIAL_FILE)).await? {
            return Err(restore_invalid(
                "The Knowledge restore preflight found a partial candidate.",
            ));
        }
        let candidate_exists = optional_regular_file(&candidate).await?;
        if !matches!(
            inspect_live_payload_layout(
                &self.adapter,
                &self.snapshot.manifest.binding.installation,
            )
            .await?,
            LiveKnowledgePayloadLayout::Absent
        ) {
            return Err(restore_target_not_empty());
        }
        match (&self.snapshot.manifest.payload, &self.candidate) {
            (ControlKnowledgePayloadState::Absent, None) => {
                if candidate_exists {
                    return Err(restore_invalid(
                        "An absent Knowledge snapshot has staged database bytes.",
                    ));
                }
                Ok(())
            }
            (ControlKnowledgePayloadState::Archive { backup, .. }, Some(expected))
                if expected == &candidate && candidate_exists =>
            {
                validate_inventory(
                    &self.adapter,
                    &candidate,
                    backup,
                    &self.bindings,
                    &self.selected,
                )
                .await
            }
            _ => Err(restore_invalid(
                "The staged Knowledge payload differs from its clean restore target.",
            )),
        }
    }
}
