use a3s_use_core::{OkfProjectionReceipt, PlanScope, UseError, UseResult};
use rusqlite::{params, Connection};
use serde::Serialize;

use super::policy::{
    OkfKnowledgeStoragePolicy, MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS,
    MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES,
};
use super::projection::{database_invalid, database_io, load_projection, ProjectionState};

const MAX_RETAINED_RECEIPT_BYTES: usize = 256 * 1024;

/// Non-secret, scope-local Knowledge storage evidence.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OkfKnowledgeStorageUsage {
    pub scope: PlanScope,
    pub retained_projections: usize,
    pub removed_tombstones: usize,
    pub retained_expanded_bytes: u64,
    pub max_scope_projections: usize,
    pub max_scope_expanded_bytes: u64,
    pub max_surface_generations: usize,
    pub max_scope_tombstones: usize,
    pub database_bytes: u64,
    pub reclaimable_database_bytes: u64,
}

impl OkfKnowledgeStorageUsage {
    pub(super) fn empty(scope: PlanScope, policy: &OkfKnowledgeStoragePolicy) -> Self {
        Self {
            scope,
            retained_projections: 0,
            removed_tombstones: 0,
            retained_expanded_bytes: 0,
            max_scope_projections: policy.max_scope_projections(),
            max_scope_expanded_bytes: policy.max_scope_expanded_bytes(),
            max_surface_generations: policy.max_surface_generations(),
            max_scope_tombstones: policy.max_scope_tombstones(),
            database_bytes: 0,
            reclaimable_database_bytes: 0,
        }
    }
}

pub(super) fn usage(
    connection: &Connection,
    scope: &PlanScope,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<OkfKnowledgeStorageUsage> {
    let mut query = connection
        .prepare(
            "SELECT package_id, surface_id, generation, receipt_json, receipt_digest
             FROM knowledge_projections
             ORDER BY package_id, surface_id, generation",
        )
        .map_err(|error| database_io("prepare Knowledge storage accounting", error))?;
    let mut rows = query
        .query([])
        .map_err(|error| database_io("query Knowledge storage accounting", error))?;
    let mut retained_projections = 0_usize;
    let mut removed_tombstones = 0_usize;
    let mut retained_expanded_bytes = 0_u64;
    let mut total = 0_usize;
    let hard_row_limit = MAX_OKF_KNOWLEDGE_SCOPE_PROJECTIONS
        .checked_add(MAX_OKF_KNOWLEDGE_SCOPE_TOMBSTONES)
        .ok_or_else(|| database_invalid("The Knowledge storage row bound overflowed."))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| database_io("read Knowledge storage accounting", error))?
    {
        total = total.saturating_add(1);
        if total > hard_row_limit {
            return Err(database_invalid(
                "The Knowledge database exceeds its hard projection and tombstone row bound.",
            ));
        }
        let package_id = row
            .get::<_, String>(0)
            .map_err(|error| database_io("read Knowledge accounting package identity", error))?;
        let surface_id = row
            .get::<_, String>(1)
            .map_err(|error| database_io("read Knowledge accounting surface identity", error))?;
        let generation = row
            .get::<_, i64>(2)
            .map_err(|error| database_io("read Knowledge accounting generation", error))?;
        let receipt_bytes = row
            .get::<_, Vec<u8>>(3)
            .map_err(|error| database_io("read Knowledge accounting receipt", error))?;
        let receipt_digest = row
            .get::<_, String>(4)
            .map_err(|error| database_io("read Knowledge accounting receipt digest", error))?;
        if receipt_bytes.is_empty() || receipt_bytes.len() > MAX_RETAINED_RECEIPT_BYTES {
            return Err(database_invalid(
                "A retained Knowledge projection receipt exceeds its accounting bound.",
            ));
        }
        let generation = u64::try_from(generation)
            .map_err(|_| database_invalid("A retained Knowledge generation is negative."))?;
        let stored = load_projection(connection, &package_id, &surface_id, generation)?
            .ok_or_else(|| database_invalid("A Knowledge accounting row disappeared."))?;
        let receipt = OkfProjectionReceipt::from_json(&receipt_bytes).map_err(|error| {
            database_invalid(format!(
                "A retained Knowledge accounting receipt is invalid: {}",
                error.message
            ))
        })?;
        if receipt != stored.receipt
            || receipt.scope != *scope
            || receipt.descriptor_digest()? != receipt_digest
        {
            return Err(database_invalid(
                "A retained Knowledge accounting receipt changed scope or digest evidence.",
            ));
        }
        match stored.state {
            ProjectionState::Removed => {
                removed_tombstones = removed_tombstones.saturating_add(1);
            }
            ProjectionState::Staged | ProjectionState::Promoted => {
                retained_projections = retained_projections.saturating_add(1);
                retained_expanded_bytes = retained_expanded_bytes
                    .checked_add(receipt.bundle.expanded_bytes)
                    .ok_or_else(|| {
                        database_invalid("The retained Knowledge byte accounting overflowed.")
                    })?;
            }
        }
    }
    drop(rows);
    drop(query);

    let page_size = pragma_u64(connection, "page_size")?;
    let page_count = pragma_u64(connection, "page_count")?;
    let free_pages = pragma_u64(connection, "freelist_count")?;
    if free_pages > page_count {
        return Err(database_invalid(
            "The Knowledge database freelist exceeds its allocated page count.",
        ));
    }
    Ok(OkfKnowledgeStorageUsage {
        scope: scope.clone(),
        retained_projections,
        removed_tombstones,
        retained_expanded_bytes,
        max_scope_projections: policy.max_scope_projections(),
        max_scope_expanded_bytes: policy.max_scope_expanded_bytes(),
        max_surface_generations: policy.max_surface_generations(),
        max_scope_tombstones: policy.max_scope_tombstones(),
        database_bytes: page_count
            .checked_mul(page_size)
            .ok_or_else(|| database_invalid("The Knowledge database byte count overflowed."))?,
        reclaimable_database_bytes: free_pages
            .checked_mul(page_size)
            .ok_or_else(|| database_invalid("The Knowledge freelist byte count overflowed."))?,
    })
}

pub(super) fn enforce_stage(
    usage: &OkfKnowledgeStorageUsage,
    requested_expanded_bytes: u64,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<()> {
    if usage.retained_projections >= policy.max_scope_projections() {
        return Err(UseError::new(
            "use.okf.knowledge_scope_projection_limit_exceeded",
            "The complete Knowledge scope reached its retained-projection limit; receipt-owned removal is required before another stage.",
        )
        .with_detail(
            "retainedProjections",
            serde_json::json!(usage.retained_projections),
        )
        .with_detail(
            "maxScopeProjections",
            serde_json::json!(policy.max_scope_projections()),
        ));
    }
    let next = usage
        .retained_expanded_bytes
        .checked_add(requested_expanded_bytes)
        .ok_or_else(|| database_invalid("The Knowledge scope quota calculation overflowed."))?;
    if next > policy.max_scope_expanded_bytes() {
        return Err(UseError::new(
            "use.okf.knowledge_scope_quota_exceeded",
            "The complete Knowledge scope would exceed its retained expanded-byte quota.",
        )
        .with_detail(
            "retainedExpandedBytes",
            serde_json::json!(usage.retained_expanded_bytes),
        )
        .with_detail(
            "requestedExpandedBytes",
            serde_json::json!(requested_expanded_bytes),
        )
        .with_detail(
            "maxScopeExpandedBytes",
            serde_json::json!(policy.max_scope_expanded_bytes()),
        ));
    }
    Ok(())
}

pub(super) fn prune_tombstones(
    connection: &Connection,
    max_scope_tombstones: usize,
) -> UseResult<()> {
    connection
        .execute(
            "DELETE FROM knowledge_projections
             WHERE rowid IN (
                 SELECT rowid FROM knowledge_projections
                 WHERE state = 'removed'
                 ORDER BY observed_at_ms DESC, package_id, surface_id, generation DESC
                 LIMIT -1 OFFSET ?1
             )",
            params![i64::try_from(max_scope_tombstones).map_err(|_| {
                database_invalid("The Knowledge tombstone limit exceeds SQLite bounds.")
            })?],
        )
        .map_err(|error| database_io("prune retained Knowledge tombstones", error))?;
    Ok(())
}

pub(super) fn collect_garbage(
    connection: &mut Connection,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<()> {
    prune_tombstones(connection, policy.max_scope_tombstones())?;
    connection
        .execute_batch("PRAGMA optimize; VACUUM; PRAGMA wal_checkpoint(TRUNCATE);")
        .map_err(|error| database_io("compact retired Knowledge storage", error))?;
    Ok(())
}

fn pragma_u64(connection: &Connection, name: &str) -> UseResult<u64> {
    let value = connection
        .query_row(&format!("PRAGMA {name}"), [], |row| row.get::<_, i64>(0))
        .map_err(|error| database_io("read Knowledge database page accounting", error))?;
    u64::try_from(value)
        .map_err(|_| database_invalid("The Knowledge database page accounting is negative."))
}
