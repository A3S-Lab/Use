use a3s_use_core::{UseError, UseResult};
use serde::Serialize;

pub const VERIFIED_TARGET_CACHE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES: u64 = 4 * 1024 * 1024 * 1024;
pub const DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES: u64 = 4_096;
pub const DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES: u64 = 256 * 1024 * 1024;

const MAX_VERIFIED_TARGET_CACHE_POLICY_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_VERIFIED_TARGET_CACHE_POLICY_ENTRIES: u64 = 100_000;

/// Per-Registry bounds for verified package archives and planning targets.
///
/// The policy is enforced before a target request and again while committing
/// the verified target. Oldest verified targets are removed first. Cache
/// removal never changes installed package generations or trust receipts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedTargetCachePolicy {
    max_bytes: u64,
    max_entries: u64,
    min_free_bytes: u64,
}

impl VerifiedTargetCachePolicy {
    pub fn new(max_bytes: u64, max_entries: u64, min_free_bytes: u64) -> UseResult<Self> {
        if max_bytes == 0 || max_bytes > MAX_VERIFIED_TARGET_CACHE_POLICY_BYTES {
            return Err(policy_error(format!(
                "Verified target cache max bytes must be between 1 and {MAX_VERIFIED_TARGET_CACHE_POLICY_BYTES}."
            )));
        }
        if max_entries == 0 || max_entries > MAX_VERIFIED_TARGET_CACHE_POLICY_ENTRIES {
            return Err(policy_error(format!(
                "Verified target cache max entries must be between 1 and {MAX_VERIFIED_TARGET_CACHE_POLICY_ENTRIES}."
            )));
        }
        if min_free_bytes > MAX_VERIFIED_TARGET_CACHE_POLICY_BYTES {
            return Err(policy_error(format!(
                "Verified target cache minimum free bytes cannot exceed {MAX_VERIFIED_TARGET_CACHE_POLICY_BYTES}."
            )));
        }
        max_bytes.checked_add(min_free_bytes).ok_or_else(|| {
            policy_error("The verified target cache storage policy overflows its byte bounds.")
        })?;
        Ok(Self {
            max_bytes,
            max_entries,
            min_free_bytes,
        })
    }

    pub const fn max_bytes(self) -> u64 {
        self.max_bytes
    }

    pub const fn max_entries(self) -> u64 {
        self.max_entries
    }

    pub const fn min_free_bytes(self) -> u64 {
        self.min_free_bytes
    }
}

impl Default for VerifiedTargetCachePolicy {
    fn default() -> Self {
        Self {
            max_bytes: DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES,
            max_entries: DEFAULT_VERIFIED_TARGET_CACHE_MAX_ENTRIES,
            min_free_bytes: DEFAULT_VERIFIED_TARGET_CACHE_MIN_FREE_BYTES,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedTargetCacheUsage {
    pub schema_version: u32,
    pub registry_name: String,
    pub registry_url: String,
    pub target_entries: u64,
    pub target_bytes: u64,
    pub stale_entries: u64,
    pub stale_bytes: u64,
    pub available_bytes: u64,
    pub policy: VerifiedTargetCachePolicy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VerifiedTargetCachePruneResult {
    pub schema_version: u32,
    pub before: VerifiedTargetCacheUsage,
    pub after: VerifiedTargetCacheUsage,
    pub removed_target_entries: u64,
    pub removed_target_bytes: u64,
    pub removed_stale_entries: u64,
    pub removed_stale_bytes: u64,
}

fn policy_error(message: impl Into<String>) -> UseError {
    UseError::new(
        "use.extension.registry_target_cache_policy_invalid",
        message,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_policy_is_bounded_and_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<VerifiedTargetCachePolicy>();
        assert_send_sync::<VerifiedTargetCacheUsage>();
        assert_send_sync::<VerifiedTargetCachePruneResult>();
        let policy = VerifiedTargetCachePolicy::new(1, 1, 0).unwrap();
        assert_eq!(policy.max_bytes(), 1);
        assert_eq!(policy.max_entries(), 1);
        assert_eq!(policy.min_free_bytes(), 0);
        assert_eq!(
            VerifiedTargetCachePolicy::new(0, 1, 0).unwrap_err().code,
            "use.extension.registry_target_cache_policy_invalid"
        );
        assert_eq!(
            VerifiedTargetCachePolicy::new(1, 0, 0).unwrap_err().code,
            "use.extension.registry_target_cache_policy_invalid"
        );
    }
}
