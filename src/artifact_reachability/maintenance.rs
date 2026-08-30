use std::collections::BTreeSet;

use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::{ArtifactCollectionGuard, ArtifactKind, UsePaths};

use super::ArtifactReachabilityInspector;

/// Global coordinator for reference-aware Artifact Store maintenance.
///
/// The root facade owns the complete logical reference scan; the extension
/// crate owns physical Artifact Store mutation. Keeping one collection guard
/// across both responsibilities prevents a zero-reference proof from becoming
/// stale.
#[derive(Debug, Clone)]
pub struct ArtifactStoreMaintenance {
    pub(super) inspector: ArtifactReachabilityInspector,
}

pub(super) struct TargetReferenceSummary {
    pub(super) reference_count: u64,
    pub(super) owner_groups: u64,
    pub(super) referenced_targets: u64,
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

    pub(super) async fn target_reference_summary(
        &self,
        collection: &ArtifactCollectionGuard,
        targets: &BTreeSet<(ArtifactKind, String)>,
    ) -> UseResult<TargetReferenceSummary> {
        let references = self
            .inspector
            .inspect_references_under_collection(collection)
            .await?;
        let mut reference_count = 0_u64;
        let mut owner_groups = 0_u64;
        let mut referenced_targets = BTreeSet::new();
        for reference in &references.entries {
            let target = (reference.kind, reference.digest.clone());
            if targets.contains(&target) {
                reference_count = reference_count
                    .checked_add(reference.reference_count)
                    .ok_or_else(reference_count_overflow)?;
                owner_groups = owner_groups
                    .checked_add(1)
                    .ok_or_else(reference_count_overflow)?;
                referenced_targets.insert(target);
            }
        }
        Ok(TargetReferenceSummary {
            reference_count,
            owner_groups,
            referenced_targets: u64::try_from(referenced_targets.len())
                .map_err(|_| reference_count_overflow())?,
        })
    }
}

fn reference_count_overflow() -> UseError {
    UseError::new(
        "use.artifact_maintenance.reference_limit_exceeded",
        "Artifact Store maintenance reference evidence exceeds its numeric bound.",
    )
}
