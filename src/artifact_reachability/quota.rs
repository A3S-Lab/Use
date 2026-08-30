use a3s_use_core::UseResult;
pub use a3s_use_extension::{ArtifactStorageQuotaPolicy, MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS};
use serde::Serialize;

use super::joined::{validated_usage, ArtifactReachabilityInventory, ArtifactStorageUsage};

/// Quota projection over one verified joined inventory. Unreferenced evidence
/// is reported for planning only and is not equivalent to reclaimable space.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStorageQuotaAssessment {
    pub policy: ArtifactStorageQuotaPolicy,
    pub usage: ArtifactStorageUsage,
    pub within_quota: bool,
    pub excess_bytes: u64,
    pub excess_artifacts: u64,
}

impl ArtifactReachabilityInventory {
    pub fn assess_quota(
        &self,
        policy: ArtifactStorageQuotaPolicy,
    ) -> UseResult<ArtifactStorageQuotaAssessment> {
        let usage = validated_usage(self)?;
        let excess_bytes = usage
            .physical_bytes
            .saturating_sub(policy.max_physical_bytes());
        let excess_artifacts = usage
            .physical_artifacts
            .saturating_sub(policy.max_physical_artifacts());
        Ok(ArtifactStorageQuotaAssessment {
            policy,
            usage,
            within_quota: excess_bytes == 0 && excess_artifacts == 0,
            excess_bytes,
            excess_artifacts,
        })
    }
}
