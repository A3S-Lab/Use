use std::collections::BTreeSet;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::ArtifactCollectionGuard;
pub use a3s_use_extension::{
    ArtifactGarbageCollectionEntry, ArtifactGarbageCollectionLifecycle,
    ArtifactGarbageCollectionPlan, ArtifactGarbageCollectionPolicy,
    ArtifactGarbageCollectionRecord, ArtifactGarbageCollectionResult,
    ArtifactGarbageCollectionTarget, ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA,
    ARTIFACT_GARBAGE_COLLECTION_RECORD_SCHEMA, ARTIFACT_GARBAGE_COLLECTION_RESULT_SCHEMA,
    MAX_ARTIFACT_GARBAGE_COLLECTION_TARGETS,
};

use super::maintenance::ArtifactStoreMaintenance;

impl ArtifactStoreMaintenance {
    /// Prove that every explicitly selected target has zero durable owners,
    /// then derive exact path-free physical deletion evidence under the same
    /// global collection guard.
    pub async fn plan_garbage_collection(
        &self,
        policy: ArtifactGarbageCollectionPolicy,
    ) -> UseResult<ArtifactGarbageCollectionPlan> {
        policy.validate()?;
        let store = self.inspector.paths.artifact_store();
        let collection = store.acquire_collection().await?;
        self.require_targets_unreferenced(&collection, &policy)
            .await?;
        store
            .plan_physical_garbage_collection(&collection, policy)
            .await
    }

    /// Re-prove zero durable owners and apply or resume only the exact
    /// confirmed deletion plan. A matching durable completion is replayed
    /// read-only before current references are inspected.
    pub async fn apply_garbage_collection(
        &self,
        policy: ArtifactGarbageCollectionPolicy,
        expected_plan_digest: &str,
    ) -> UseResult<ArtifactGarbageCollectionResult> {
        policy.validate()?;
        let store = self.inspector.paths.artifact_store();
        let collection = store.acquire_collection().await?;
        if let Some(result) = store
            .replay_completed_garbage_collection(&collection, &policy, expected_plan_digest)
            .await?
        {
            return Ok(result);
        }
        self.require_targets_unreferenced(&collection, &policy)
            .await?;
        store
            .apply_unreferenced_garbage_collection(&collection, policy, expected_plan_digest)
            .await
    }

    async fn require_targets_unreferenced(
        &self,
        collection: &ArtifactCollectionGuard,
        policy: &ArtifactGarbageCollectionPolicy,
    ) -> UseResult<()> {
        policy.validate()?;
        let targets = policy
            .targets
            .iter()
            .map(|target| (target.kind, target.digest.clone()))
            .collect::<BTreeSet<_>>();
        let summary = self.target_reference_summary(collection, &targets).await?;
        if summary.reference_count != 0 {
            return Err(UseError::new(
                "use.artifact_garbage_collection.referenced",
                "Artifact Store garbage collection cannot delete targets with durable owners.",
            )
            .with_detail("referenceCount", summary.reference_count.to_string())
            .with_detail("ownerGroups", summary.owner_groups.to_string())
            .with_detail("referencedTargets", summary.referenced_targets.to_string())
            .with_suggestion(
                "Retire every Registry observation, installation receipt, snapshot, and nonterminal operation for each target before creating a new deletion plan.",
            ));
        }
        Ok(())
    }
}
