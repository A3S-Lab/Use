use a3s_use_core::{
    OkfKnowledgeObservation, OkfProjectionReceipt, UseError, UseResult,
    OKF_PROJECTION_RECEIPT_SCHEMA,
};
use rusqlite::{params, Connection, TransactionBehavior};

use super::index::{PreparedIndex, INDEX_SCHEMA};
use super::policy::OkfKnowledgeStoragePolicy;
use super::projection::{
    advancing_timestamp, database_conflict, database_io, generation_i64, load_projection,
    observation, projection_id, require_projection, selected_generation, timestamp_i64,
    validate_stage_replay, ProjectionState,
};
use super::storage;
use crate::okf_knowledge::{OkfKnowledgeBinding, OkfKnowledgeStageSpec};

pub(super) fn stage(
    connection: &mut Connection,
    spec: &OkfKnowledgeStageSpec,
    index: &PreparedIndex,
    now_ms: u64,
    policy: &OkfKnowledgeStoragePolicy,
) -> UseResult<OkfKnowledgeBinding> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| database_io("begin stage transaction", error))?;
    if let Some(existing) = load_projection(
        &transaction,
        &spec.surface.package_id,
        &spec.surface.surface.id,
        spec.generation,
    )? {
        validate_stage_replay(&existing, spec, index)?;
        if existing.state != ProjectionState::Staged {
            return Err(database_conflict(
                "The exact OKF generation was already promoted or removed and cannot be staged again.",
            ));
        }
        let selected = selected_generation(
            &transaction,
            &spec.surface.package_id,
            &spec.surface.surface.id,
        )?;
        let observation = observation(
            &existing.receipt,
            existing.state,
            &existing.index_digest,
            existing.observed_at_ms,
            selected,
        )?;
        transaction
            .commit()
            .map_err(|error| database_io("commit idempotent stage transaction", error))?;
        return OkfKnowledgeBinding::new(existing.receipt, observation);
    }

    storage::prune_tombstones(&transaction, policy.max_scope_tombstones())?;
    let usage = storage::usage(&transaction, &spec.scope, policy)?;
    storage::enforce_stage(&usage, spec.bundle.expanded_bytes, policy)?;
    let retained: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM knowledge_projections
             WHERE package_id = ?1 AND surface_id = ?2 AND state != 'removed'",
            params![spec.surface.package_id, spec.surface.surface.id],
            |row| row.get(0),
        )
        .map_err(|error| database_io("count retained Knowledge generations", error))?;
    if retained >= policy.max_surface_generations() as i64 {
        return Err(UseError::new(
            "use.okf.knowledge_database_generation_limit",
            format!(
                "The Knowledge surface reached its retained-generation limit of {}; receipt-owned removal is required before another stage.",
                policy.max_surface_generations()
            ),
        ));
    }

    let receipt = OkfProjectionReceipt {
        schema: OKF_PROJECTION_RECEIPT_SCHEMA.to_owned(),
        operation_id: spec.operation_id.clone(),
        scope: spec.scope.clone(),
        surface: spec.surface.clone(),
        generation: spec.generation,
        package_digest: spec.package_digest.clone(),
        manifest_digest: spec.manifest_digest.clone(),
        bundle: spec.bundle.clone(),
        projection_id: projection_id(spec),
        index_schema: INDEX_SCHEMA.to_owned(),
        index_build_id: index.build_id.clone(),
        staged_at_ms: now_ms,
    };
    receipt.validate()?;
    let receipt_bytes = receipt.canonical_bytes()?;
    let receipt_digest = receipt.descriptor_digest()?;
    let generation = generation_i64(spec.generation)?;
    transaction
        .execute(
            "INSERT INTO knowledge_projections (
                package_id, surface_id, generation, receipt_json, receipt_digest,
                index_digest, state, staged_at_ms, observed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'staged', ?7, ?7)",
            params![
                spec.surface.package_id,
                spec.surface.surface.id,
                generation,
                receipt_bytes,
                receipt_digest,
                index.digest,
                timestamp_i64(now_ms)?,
            ],
        )
        .map_err(|error| database_io("insert staged Knowledge projection", error))?;
    {
        let mut insert = transaction
            .prepare(
                "INSERT INTO knowledge_documents (
                    package_id, surface_id, generation, concept_id, path,
                    type_name, title, search_text, source_digest
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            )
            .map_err(|error| database_io("prepare Knowledge document insertion", error))?;
        for document in &index.documents {
            insert
                .execute(params![
                    spec.surface.package_id,
                    spec.surface.surface.id,
                    generation,
                    document.concept_id,
                    document.path,
                    document.type_name,
                    document.title,
                    document.search_text,
                    document.source_digest,
                ])
                .map_err(|error| database_io("insert Knowledge search document", error))?;
        }
    }
    let selected = selected_generation(
        &transaction,
        &spec.surface.package_id,
        &spec.surface.surface.id,
    )?;
    let observation = observation(
        &receipt,
        ProjectionState::Staged,
        &index.digest,
        now_ms,
        selected,
    )?;
    transaction
        .commit()
        .map_err(|error| database_io("commit staged Knowledge projection", error))?;
    OkfKnowledgeBinding::new(receipt, observation)
}

pub(super) fn promote(
    connection: &mut Connection,
    receipt: &OkfProjectionReceipt,
    now_ms: u64,
) -> UseResult<OkfKnowledgeObservation> {
    receipt.validate()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| database_io("begin promote transaction", error))?;
    let stored = require_projection(&transaction, receipt)?;
    if stored.state == ProjectionState::Removed {
        return Err(database_conflict(
            "A removed OKF generation cannot be promoted again.",
        ));
    }
    let current = selected_generation(
        &transaction,
        &receipt.surface.package_id,
        &receipt.surface.surface.id,
    )?;
    if current
        .as_ref()
        .is_some_and(|selected| selected.generation > receipt.generation)
    {
        return Err(database_conflict(
            "An older OKF generation cannot replace a newer promoted selection.",
        ));
    }
    if stored.state == ProjectionState::Promoted
        && current
            .as_ref()
            .is_some_and(|selected| selected.generation == receipt.generation)
    {
        let result = observation(
            receipt,
            ProjectionState::Promoted,
            &stored.index_digest,
            stored.observed_at_ms,
            current,
        )?;
        transaction
            .commit()
            .map_err(|error| database_io("commit idempotent promote transaction", error))?;
        return Ok(result);
    }

    let observed_at_ms = advancing_timestamp(now_ms, stored.observed_at_ms)?;
    transaction
        .execute(
            "INSERT INTO knowledge_selection (
                package_id, surface_id, generation, selected_at_ms
             ) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT(package_id, surface_id) DO UPDATE SET
                generation = excluded.generation,
                selected_at_ms = excluded.selected_at_ms",
            params![
                receipt.surface.package_id,
                receipt.surface.surface.id,
                generation_i64(receipt.generation)?,
                timestamp_i64(observed_at_ms)?,
            ],
        )
        .map_err(|error| database_io("select promoted Knowledge generation", error))?;
    transaction
        .execute(
            "UPDATE knowledge_projections
             SET state = 'promoted', observed_at_ms = ?4
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3",
            params![
                receipt.surface.package_id,
                receipt.surface.surface.id,
                generation_i64(receipt.generation)?,
                timestamp_i64(observed_at_ms)?,
            ],
        )
        .map_err(|error| database_io("promote Knowledge generation", error))?;
    let selected = selected_generation(
        &transaction,
        &receipt.surface.package_id,
        &receipt.surface.surface.id,
    )?;
    let result = observation(
        receipt,
        ProjectionState::Promoted,
        &stored.index_digest,
        observed_at_ms,
        selected,
    )?;
    transaction
        .commit()
        .map_err(|error| database_io("commit promoted Knowledge generation", error))?;
    Ok(result)
}

pub(super) fn observe(
    connection: &Connection,
    receipt: &OkfProjectionReceipt,
) -> UseResult<OkfKnowledgeObservation> {
    receipt.validate()?;
    let stored = require_projection(connection, receipt)?;
    let selected = selected_generation(
        connection,
        &receipt.surface.package_id,
        &receipt.surface.surface.id,
    )?;
    observation(
        receipt,
        stored.state,
        &stored.index_digest,
        stored.observed_at_ms,
        selected,
    )
}

pub(super) fn remove(
    connection: &mut Connection,
    receipt: &OkfProjectionReceipt,
    now_ms: u64,
) -> UseResult<OkfKnowledgeObservation> {
    receipt.validate()?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| database_io("begin remove transaction", error))?;
    let stored = require_projection(&transaction, receipt)?;
    if stored.state == ProjectionState::Removed {
        let result = observation(
            receipt,
            ProjectionState::Removed,
            &stored.index_digest,
            stored.observed_at_ms,
            None,
        )?;
        transaction
            .commit()
            .map_err(|error| database_io("commit idempotent remove transaction", error))?;
        return Ok(result);
    }

    transaction
        .execute(
            "DELETE FROM knowledge_selection
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3",
            params![
                receipt.surface.package_id,
                receipt.surface.surface.id,
                generation_i64(receipt.generation)?,
            ],
        )
        .map_err(|error| database_io("clear removed Knowledge selection", error))?;
    transaction
        .execute(
            "DELETE FROM knowledge_documents
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3",
            params![
                receipt.surface.package_id,
                receipt.surface.surface.id,
                generation_i64(receipt.generation)?,
            ],
        )
        .map_err(|error| database_io("delete receipt-owned Knowledge documents", error))?;
    let observed_at_ms = advancing_timestamp(now_ms, stored.observed_at_ms)?;
    transaction
        .execute(
            "UPDATE knowledge_projections
             SET state = 'removed', observed_at_ms = ?4
             WHERE package_id = ?1 AND surface_id = ?2 AND generation = ?3",
            params![
                receipt.surface.package_id,
                receipt.surface.surface.id,
                generation_i64(receipt.generation)?,
                timestamp_i64(observed_at_ms)?,
            ],
        )
        .map_err(|error| database_io("tombstone removed Knowledge projection", error))?;
    let result = observation(
        receipt,
        ProjectionState::Removed,
        &stored.index_digest,
        observed_at_ms,
        None,
    )?;
    transaction
        .commit()
        .map_err(|error| database_io("commit removed Knowledge projection", error))?;
    Ok(result)
}
