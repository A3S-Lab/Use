use a3s_use_core::{UseError, UseResult};

use crate::okf_knowledge::MAX_OKF_KNOWLEDGE_GENERATIONS;

pub const DEFAULT_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
pub const DEFAULT_OKF_KNOWLEDGE_SCOPE_PROJECTIONS: usize = 256;
pub const MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS: usize = 1024;
pub const DEFAULT_OKF_KNOWLEDGE_SCOPE_TOMBSTONES: usize = 256;
pub const MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES: usize = 1024;

/// Host-owned storage limits for one complete User or Workspace Knowledge scope.
///
/// Expanded-byte accounting uses the exact immutable OKF bundle evidence in
/// each retained projection receipt. It is therefore deterministic across
/// platforms and restarts and cannot be changed independently of the receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OkfKnowledgeStoragePolicy {
    max_scope_expanded_bytes: u64,
    max_scope_projections: usize,
    max_surface_generations: usize,
    max_scope_tombstones: usize,
}

impl OkfKnowledgeStoragePolicy {
    pub fn new(
        max_scope_expanded_bytes: u64,
        max_scope_projections: usize,
        max_surface_generations: usize,
        max_scope_tombstones: usize,
    ) -> UseResult<Self> {
        if max_scope_expanded_bytes == 0
            || max_scope_expanded_bytes > MAX_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES
            || max_scope_projections == 0
            || max_scope_projections > MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS
            || max_surface_generations == 0
            || max_surface_generations > MAX_OKF_KNOWLEDGE_GENERATIONS
            || max_surface_generations > max_scope_projections
            || max_scope_tombstones == 0
            || max_scope_tombstones > MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES
        {
            return Err(UseError::new(
                "use.okf.knowledge_storage_policy_invalid",
                "The OKF Knowledge storage policy is zero, inconsistent, or exceeds its hard safety ceiling.",
            ));
        }
        Ok(Self {
            max_scope_expanded_bytes,
            max_scope_projections,
            max_surface_generations,
            max_scope_tombstones,
        })
    }

    pub const fn max_scope_expanded_bytes(&self) -> u64 {
        self.max_scope_expanded_bytes
    }

    pub const fn max_scope_projections(&self) -> usize {
        self.max_scope_projections
    }

    pub const fn max_surface_generations(&self) -> usize {
        self.max_surface_generations
    }

    pub const fn max_scope_tombstones(&self) -> usize {
        self.max_scope_tombstones
    }
}

impl Default for OkfKnowledgeStoragePolicy {
    fn default() -> Self {
        Self {
            max_scope_expanded_bytes: DEFAULT_OKF_KNOWLEDGE_SCOPE_EXPANDED_BYTES,
            max_scope_projections: DEFAULT_OKF_KNOWLEDGE_SCOPE_PROJECTIONS,
            max_surface_generations: MAX_OKF_KNOWLEDGE_GENERATIONS,
            max_scope_tombstones: DEFAULT_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
        }
    }
}
