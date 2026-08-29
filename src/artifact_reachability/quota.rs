use a3s_use_core::{UseError, UseResult};
use a3s_use_extension::MAX_ARTIFACT_STORE_INVENTORY_ENTRIES;
use serde::Serialize;

use super::joined::{validated_usage, ArtifactReachabilityInventory, ArtifactStorageUsage};

pub const MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS: u64 = MAX_ARTIFACT_STORE_INVENTORY_ENTRIES as u64;

/// Operator-selected ceiling for assessing one immutable global inventory.
/// Bytes are the inventory's logical file lengths, not allocated disk blocks.
/// Assessment is evidence only; this policy does not reserve concurrent space
/// or authorize deletion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactStorageQuotaPolicy {
    max_physical_bytes: u64,
    max_physical_artifacts: u64,
}

impl ArtifactStorageQuotaPolicy {
    pub fn new(max_physical_bytes: u64, max_physical_artifacts: u64) -> UseResult<Self> {
        if max_physical_bytes == 0
            || max_physical_artifacts == 0
            || max_physical_artifacts > MAX_ARTIFACT_STORAGE_QUOTA_ARTIFACTS
        {
            return Err(quota_invalid(
                "Artifact storage quota bounds are zero or the artifact limit exceeds the inventory ceiling.",
            ));
        }
        Ok(Self {
            max_physical_bytes,
            max_physical_artifacts,
        })
    }

    pub const fn max_physical_bytes(&self) -> u64 {
        self.max_physical_bytes
    }

    pub const fn max_physical_artifacts(&self) -> u64 {
        self.max_physical_artifacts
    }
}

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

fn quota_invalid(message: impl Into<String>) -> UseError {
    UseError::new("use.artifact_reachability.quota_policy_invalid", message)
}
