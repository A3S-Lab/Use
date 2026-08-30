use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ArtifactCollectionGuard, ArtifactKind, UsePaths};
pub use a3s_use_extension::{
    ArtifactRehydrationPlan, ArtifactRehydrationRecord, ArtifactRehydrationResult,
    ARTIFACT_REHYDRATION_PLAN_SCHEMA, ARTIFACT_REHYDRATION_RECORD_SCHEMA,
    ARTIFACT_REHYDRATION_RESULT_SCHEMA,
};

use super::ArtifactReachabilityInspector;

/// Global coordinator for reference-aware Artifact Store maintenance.
///
/// The root facade owns the complete logical reference scan; the extension
/// crate owns physical Artifact Store mutation. Keeping the collection guard
/// across both steps prevents a zero-reference proof from becoming stale.
#[derive(Debug, Clone)]
pub struct ArtifactStoreMaintenance {
    inspector: ArtifactReachabilityInspector,
}

impl ArtifactStoreMaintenance {
    pub fn from_env() -> UseResult<Self> {
        Ok(Self::new(UsePaths::from_env()?))
    }

    pub fn new(paths: UsePaths) -> Self {
        Self {
            inspector: ArtifactReachabilityInspector::new(paths),
        }
    }

    /// Verify that the target has no durable owners, then derive exact,
    /// path-free recovery evidence from an independently supplied candidate.
    pub async fn plan_rehydration(
        &self,
        kind: ArtifactKind,
        digest: &str,
        candidate: &Path,
    ) -> UseResult<ArtifactRehydrationPlan> {
        let store = self.inspector.paths.artifact_store();
        let collection = store.acquire_collection().await?;
        self.require_unreferenced(&collection, kind, digest).await?;
        store
            .plan_rehydration(&collection, kind, digest, candidate)
            .await
    }

    /// Re-prove zero durable owners under the same guard, reverify the
    /// candidate, and apply only the exact reviewed plan digest.
    pub async fn apply_rehydration(
        &self,
        kind: ArtifactKind,
        digest: &str,
        candidate: &Path,
        expected_plan_digest: &str,
    ) -> UseResult<ArtifactRehydrationResult> {
        let store = self.inspector.paths.artifact_store();
        let collection = store.acquire_collection().await?;
        if let Some(result) = store
            .replay_completed_rehydration(&collection, kind, digest, expected_plan_digest)
            .await?
        {
            return Ok(result);
        }
        self.require_unreferenced(&collection, kind, digest).await?;
        store
            .apply_unreferenced_rehydration(
                &collection,
                kind,
                digest,
                candidate,
                expected_plan_digest,
            )
            .await
    }

    async fn require_unreferenced(
        &self,
        collection: &ArtifactCollectionGuard,
        kind: ArtifactKind,
        digest: &str,
    ) -> UseResult<()> {
        let references = self
            .inspector
            .inspect_references_under_collection(collection)
            .await?;
        let mut reference_count = 0_u64;
        let mut owner_groups = 0_u64;
        for reference in references
            .entries
            .iter()
            .filter(|entry| entry.kind == kind && entry.digest == digest)
        {
            reference_count = reference_count
                .checked_add(reference.reference_count)
                .ok_or_else(reference_count_overflow)?;
            owner_groups = owner_groups
                .checked_add(1)
                .ok_or_else(reference_count_overflow)?;
        }
        if reference_count != 0 {
            return Err(UseError::new(
                "use.artifact_rehydration.referenced",
                "A quarantined artifact cannot be rehydrated while durable references still own its canonical content.",
            )
            .with_detail("kind", kind_name(kind))
            .with_detail("digest", digest.to_owned())
            .with_detail("referenceCount", reference_count.to_string())
            .with_detail("ownerGroups", owner_groups.to_string())
            .with_suggestion(
                "Retire every Registry observation, installation receipt, snapshot, and nonterminal operation for this artifact before creating a new recovery plan.",
            ));
        }
        Ok(())
    }
}

fn kind_name(kind: ArtifactKind) -> &'static str {
    match kind {
        ArtifactKind::Blob => "blob",
        ArtifactKind::ExpandedPackage => "expanded-package",
    }
}

fn reference_count_overflow() -> UseError {
    UseError::new(
        "use.artifact_rehydration.reference_limit_exceeded",
        "Artifact rehydration reference evidence exceeds its numeric bound.",
    )
}
