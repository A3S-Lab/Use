use std::collections::BTreeMap;
use std::path::PathBuf;

use a3s_use_core::UseResult;

use super::io::{
    remove_completed_record, remove_prepared_record, write_completed_record, write_prepared_record,
    GarbageCollectionState,
};
use super::{
    deletion, garbage_collection_entry, garbage_collection_in_progress,
    garbage_collection_plan_invalid, garbage_collection_result, garbage_collection_state_invalid,
    inspect_garbage_collection_state, record_for_plan, require_record_confirmation,
    validate_expected_plan_digest, ArtifactGarbageCollectionEntry,
    ArtifactGarbageCollectionLifecycle, ArtifactGarbageCollectionPlan,
    ArtifactGarbageCollectionPolicy, ArtifactGarbageCollectionResult,
    ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA,
};
use crate::artifact_store::quarantine::ContainerQuarantineState;
use crate::artifact_store::rehydration::ContainerRehydrationState;
use crate::artifact_store::{
    validate_sha256, ArtifactCollectionGuard, ArtifactKind, ArtifactStore,
};

impl ArtifactStore {
    /// Derive exact physical evidence for an explicit target allowlist.
    ///
    /// This hidden physical seam does not scan logical owners. The root A3S
    /// Use maintenance coordinator must prove zero references under the same
    /// collection guard before exposing this plan.
    #[doc(hidden)]
    pub async fn plan_physical_garbage_collection(
        &self,
        collection: &ArtifactCollectionGuard,
        policy: ArtifactGarbageCollectionPolicy,
    ) -> UseResult<ArtifactGarbageCollectionPlan> {
        collection.ensure_store(self)?;
        let predecessor = match inspect_garbage_collection_state(self.root()).await? {
            GarbageCollectionState::None => None,
            GarbageCollectionState::Completed { record, .. } => Some(record.plan_digest),
            _ => return Err(garbage_collection_in_progress()),
        };
        self.build_garbage_collection_plan(collection, policy, predecessor)
            .await
    }

    /// Return a durable terminal outcome without reopening deleted paths.
    #[doc(hidden)]
    pub async fn replay_completed_garbage_collection(
        &self,
        collection: &ArtifactCollectionGuard,
        policy: &ArtifactGarbageCollectionPolicy,
        expected_plan_digest: &str,
    ) -> UseResult<Option<ArtifactGarbageCollectionResult>> {
        collection.ensure_store(self)?;
        policy.validate()?;
        validate_expected_plan_digest(expected_plan_digest)?;
        let GarbageCollectionState::Completed { record, .. } =
            inspect_garbage_collection_state(self.root()).await?
        else {
            return Ok(None);
        };
        if record.plan_digest != expected_plan_digest {
            return Ok(None);
        }
        if &record.plan.policy != policy {
            return Err(super::garbage_collection_plan_mismatch(
                "The completed Artifact Store garbage collection differs from the confirmed policy.",
            ));
        }
        deletion::require_no_tombstones(self, &record).await?;
        garbage_collection_result(record, false).map(Some)
    }

    /// Apply or resume one exact plan after the root coordinator has proven
    /// that every target has zero durable references under `collection`.
    #[doc(hidden)]
    pub async fn apply_unreferenced_garbage_collection(
        &self,
        collection: &ArtifactCollectionGuard,
        policy: ArtifactGarbageCollectionPolicy,
        expected_plan_digest: &str,
    ) -> UseResult<ArtifactGarbageCollectionResult> {
        collection.ensure_store(self)?;
        policy.validate()?;
        validate_expected_plan_digest(expected_plan_digest)?;
        if let Some(result) = self
            .replay_completed_garbage_collection(collection, &policy, expected_plan_digest)
            .await?
        {
            return Ok(result);
        }

        let state = inspect_garbage_collection_state(self.root()).await?;
        let (record, recover_preparation, predecessor) = match state {
            GarbageCollectionState::None => {
                let plan = self
                    .build_garbage_collection_plan(collection, policy.clone(), None)
                    .await?;
                (record_for_plan(plan, expected_plan_digest)?, false, None)
            }
            GarbageCollectionState::InterruptedPreparation { predecessor } => {
                let predecessor_digest = predecessor
                    .as_ref()
                    .map(|record| record.plan_digest.clone());
                let plan = self
                    .build_garbage_collection_plan(collection, policy.clone(), predecessor_digest)
                    .await?;
                (
                    record_for_plan(plan, expected_plan_digest)?,
                    true,
                    predecessor,
                )
            }
            GarbageCollectionState::Completed {
                record,
                prepared_record_present,
            } => {
                if prepared_record_present {
                    remove_prepared_record(self.root()).await?;
                }
                let plan = self
                    .build_garbage_collection_plan(
                        collection,
                        policy.clone(),
                        Some(record.plan_digest.clone()),
                    )
                    .await?;
                (
                    record_for_plan(plan, expected_plan_digest)?,
                    false,
                    Some(record),
                )
            }
            GarbageCollectionState::Prepared {
                record,
                predecessor,
            } => {
                require_record_confirmation(&record, &policy, expected_plan_digest)?;
                (record, false, predecessor)
            }
            GarbageCollectionState::InterruptedCompletion { record } => {
                require_record_confirmation(&record, &policy, expected_plan_digest)?;
                (record, false, None)
            }
        };

        if !matches!(
            inspect_garbage_collection_state(self.root()).await?,
            GarbageCollectionState::Prepared { .. }
                | GarbageCollectionState::InterruptedCompletion { .. }
        ) {
            write_prepared_record(self.root(), &record, recover_preparation).await?;
        }
        if let Some(predecessor) = predecessor {
            if record.plan.predecessor_plan_digest.as_deref()
                != Some(predecessor.plan_digest.as_str())
            {
                return Err(garbage_collection_state_invalid(
                    "The active garbage-collection plan is not chained to its predecessor.",
                ));
            }
            remove_completed_record(self.root()).await?;
        }

        for artifact in &record.plan.artifacts {
            deletion::retire_artifact(self, artifact, &record.plan_digest).await?;
        }

        let recover_completion = matches!(
            inspect_garbage_collection_state(self.root()).await?,
            GarbageCollectionState::InterruptedCompletion { .. }
        );
        write_completed_record(self.root(), &record, recover_completion).await?;
        match inspect_garbage_collection_state(self.root()).await? {
            GarbageCollectionState::Completed {
                record: observed, ..
            } if observed == record => {}
            _ => {
                return Err(garbage_collection_state_invalid(
                    "Artifact Store garbage collection did not durably complete as reviewed.",
                ))
            }
        }
        remove_prepared_record(self.root()).await?;
        deletion::require_no_tombstones(self, &record).await?;
        garbage_collection_result(record, true)
    }

    async fn build_garbage_collection_plan(
        &self,
        collection: &ArtifactCollectionGuard,
        policy: ArtifactGarbageCollectionPolicy,
        predecessor_plan_digest: Option<String>,
    ) -> UseResult<ArtifactGarbageCollectionPlan> {
        policy.validate()?;
        let inventory = self.inspect_inventory(collection).await?;
        let physical = inventory
            .entries
            .into_iter()
            .map(|entry| ((entry.kind, entry.digest.clone()), entry))
            .collect::<BTreeMap<_, _>>();
        let mut artifacts = Vec::with_capacity(policy.targets.len());
        let mut reclaimable_bytes = 0_u64;
        for target in &policy.targets {
            let entry = physical
                .get(&(target.kind, target.digest.clone()))
                .ok_or_else(|| {
                    garbage_collection_plan_invalid(
                        "An explicit garbage-collection target has no physical digest container.",
                    )
                })?;
            let lifecycle = self
                .garbage_collection_lifecycle(target.kind, &target.digest)
                .await?;
            let artifact = garbage_collection_entry(entry, lifecycle)?;
            reclaimable_bytes = reclaimable_bytes
                .checked_add(artifact.content_bytes)
                .and_then(|total| total.checked_add(artifact.staging_bytes))
                .ok_or_else(|| {
                    garbage_collection_plan_invalid(
                        "Garbage-collection plan byte accounting overflowed.",
                    )
                })?;
            artifacts.push(artifact);
        }
        let plan = ArtifactGarbageCollectionPlan {
            schema: ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA.to_owned(),
            policy,
            predecessor_plan_digest,
            artifact_count: artifacts.len() as u64,
            artifacts,
            reclaimable_bytes,
            required_reference_count: 0,
        };
        plan.validate()?;
        Ok(plan)
    }

    pub(super) async fn inspect_garbage_collection_entry(
        &self,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<ArtifactGarbageCollectionEntry> {
        let (sha256, container) = self.garbage_collection_container(kind, digest)?;
        let physical =
            crate::artifact_store::inventory::inspect_container_entry(&container, &sha256, kind)
                .await?;
        let lifecycle = self.garbage_collection_lifecycle(kind, digest).await?;
        garbage_collection_entry(&physical, lifecycle)
    }

    pub(super) fn garbage_collection_container(
        &self,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<(String, PathBuf)> {
        let sha256 = digest.strip_prefix("sha256:").ok_or_else(|| {
            garbage_collection_plan_invalid(
                "An Artifact Store garbage-collection digest must use 'sha256:'.",
            )
        })?;
        validate_sha256(sha256).map_err(|error| garbage_collection_plan_invalid(error.message))?;
        let container = match kind {
            ArtifactKind::Blob => self.blob_container(sha256),
            ArtifactKind::ExpandedPackage => self.expanded_package_container(sha256),
        };
        Ok((sha256.to_owned(), container))
    }

    async fn garbage_collection_lifecycle(
        &self,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<ArtifactGarbageCollectionLifecycle> {
        let (_, container) = self.garbage_collection_container(kind, digest)?;
        let quarantine =
            crate::artifact_store::quarantine::inspect_container_state(&container, kind, digest)
                .await?;
        let rehydration =
            crate::artifact_store::rehydration::inspect_rehydration_state(&container).await?;
        crate::artifact_store::rehydration::validate_container_rehydration_state(
            kind,
            digest,
            &quarantine,
            &rehydration,
        )?;
        match (quarantine, rehydration) {
            (ContainerQuarantineState::None, ContainerRehydrationState::None) => {
                Ok(ArtifactGarbageCollectionLifecycle::Ordinary)
            }
            (
                ContainerQuarantineState::Quarantined(quarantine),
                ContainerRehydrationState::None,
            ) => Ok(ArtifactGarbageCollectionLifecycle::Quarantined {
                quarantine_plan_digest: quarantine.plan_digest,
            }),
            (
                ContainerQuarantineState::Quarantined(quarantine),
                ContainerRehydrationState::Rehydrated(rehydration),
            ) => Ok(ArtifactGarbageCollectionLifecycle::Rehydrated {
                quarantine_plan_digest: quarantine.plan_digest,
                rehydration_plan_digest: rehydration.plan_digest,
            }),
            _ => Err(garbage_collection_state_invalid(
                "Garbage collection refuses an interrupted quarantine or rehydration lifecycle.",
            )),
        }
    }
}
