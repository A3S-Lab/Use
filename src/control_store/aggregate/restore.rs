use super::*;

pub(in crate::control_store) fn restore_export(
    path: &Path,
    installation: &InstallationId,
    export: &ControlStoreExport,
) -> UseResult<schema::ControlStoreMetadata> {
    super::super::export::validate_for_restore(export, installation)?;
    schema::initialize(path, installation)?;
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "restore Control Store export")?;
    if read_cursors(&transaction)? != (0, 0)
        || transaction
            .query_row("SELECT COUNT(*) FROM control_operation", [], |row| {
                row.get::<_, i64>(0)
            })
            .map_err(|error| schema::sqlite_error("inspect empty restore target", error))?
            != 0
    {
        return Err(conflict_error(
            "The Control Store restore target is not empty.",
        ));
    }

    for operation in &export.authority.operations {
        insert_operation_record(&transaction, operation)?;
    }
    for generation in &export.authority.generations {
        let operation = export
            .authority
            .operations
            .binary_search_by(|operation| {
                operation
                    .reviewed
                    .operation_id
                    .cmp(&generation.operation_id)
            })
            .ok()
            .map(|index| &export.authority.operations[index])
            .ok_or_else(|| corruption_error("A restored generation has no reviewed operation."))?;
        let operation_effects = export
            .authority
            .effects
            .iter()
            .filter(|effect| effect.operation_id == generation.operation_id)
            .collect::<Vec<_>>();
        insert_generation(
            &transaction,
            &ControlTransition {
                operation_id: generation.operation_id.clone(),
                plan_digest: operation.reviewed.plan_digest.clone(),
                snapshot: generation.snapshot.clone(),
                grants: generation.grants.clone(),
                bindings: generation.bindings.clone(),
                capability: generation.capability.clone(),
                effects: operation_effects
                    .iter()
                    .map(|effect| effect.intent.clone())
                    .collect(),
                committed_at_ms: generation.committed_at_ms,
            },
        )?;
        transaction
            .execute(
                "UPDATE capability_generation
                 SET publication_state = ?2, published_at_ms = ?3
                 WHERE installation_generation = ?1",
                params![
                    to_i64(generation.snapshot.generation)?,
                    generation.capability_status.as_str(),
                    generation
                        .capability_published_at_ms
                        .map(to_i64)
                        .transpose()?,
                ],
            )
            .map_err(|error| mutation_error("restore Control Store capability status", error))?;
        for effect in operation_effects {
            restore_effect_record(&transaction, effect)?;
        }
    }
    transaction
        .execute(
            "UPDATE control_installation
             SET current_generation = ?1, published_capability_generation = ?2
             WHERE singleton = 1 AND current_generation = 0
               AND published_capability_generation = 0",
            params![
                to_i64(export.current_generation)?,
                to_i64(export.published_capability_generation)?,
            ],
        )
        .map_err(|error| mutation_error("restore Control Store cursors", error))?;
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("commit Control Store restore", error))?;
    drop(connection);

    let (metadata, authority) = export_snapshot(path, installation)?;
    if metadata.current_generation != export.current_generation
        || metadata.published_capability_generation != export.published_capability_generation
        || authority != export.authority
    {
        return Err(corruption_error(
            "The restored Control Store does not match its verified export.",
        ));
    }
    checkpoint_restore(path, installation)?;
    Ok(metadata)
}

fn insert_operation_record(
    transaction: &Transaction<'_>,
    operation: &ControlOperationRecord,
) -> UseResult<()> {
    validate_operation_record(operation)?;
    transaction
        .execute(
            "INSERT INTO control_operation(
                operation_id, plan_digest, authorization_digest, action, root_package_id,
                expected_generation, target_generation,
                expected_capability_generation, target_capability_generation,
                reviewed_at_ms, status, committed_at_ms, completed_at_ms,
                result_digest
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
            params![
                operation.reviewed.operation_id,
                operation.reviewed.plan_digest,
                operation.reviewed.authorization_digest,
                operation_action_name(operation.reviewed.action),
                operation.reviewed.root_package_id.as_str(),
                to_i64(operation.reviewed.expected_generation)?,
                to_i64(operation.reviewed.target_generation()?)?,
                to_i64(operation.reviewed.expected_capability_generation)?,
                to_i64(operation.reviewed.target_capability_generation()?)?,
                to_i64(operation.reviewed.reviewed_at_ms)?,
                match operation.status {
                    ControlOperationStatus::Reviewed => "reviewed",
                    ControlOperationStatus::EffectsPending => "effects-pending",
                    ControlOperationStatus::Completed => "completed",
                    ControlOperationStatus::Rejected => "rejected",
                    ControlOperationStatus::Cancelled => "cancelled",
                },
                operation.committed_at_ms.map(to_i64).transpose()?,
                operation.completed_at_ms.map(to_i64).transpose()?,
                operation.result_digest,
            ],
        )
        .map_err(|error| mutation_error("restore Control Store operation", error))?;
    Ok(())
}

fn restore_effect_record(
    transaction: &Transaction<'_>,
    effect: &ControlEffectRecord,
) -> UseResult<()> {
    transaction
        .execute(
            "UPDATE effect_outbox
             SET status = ?2, attempt = ?3, claim_owner = ?4, claim_token = ?5,
                 lease_until_ms = ?6, evidence_digest = ?7, error_code = ?8,
                 observed_at_ms = ?9
             WHERE idempotency_key = ?1",
            params![
                effect.intent.idempotency_key,
                effect.status.as_str(),
                i64::from(effect.attempt),
                effect.claim_owner,
                effect.claim_token,
                effect.lease_until_ms.map(to_i64).transpose()?,
                effect.evidence_digest,
                effect.error_code,
                effect.observed_at_ms.map(to_i64).transpose()?,
            ],
        )
        .map_err(|error| mutation_error("restore Control Store outbox effect", error))?;
    Ok(())
}

fn checkpoint_restore(path: &Path, installation: &InstallationId) -> UseResult<()> {
    let connection = schema::open_verified_write(path, installation)?;
    let (busy, _, _): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| schema::sqlite_error("checkpoint restored Control Store", error))?;
    if busy != 0 {
        return Err(conflict_error(
            "The restored Control Store could not checkpoint its WAL.",
        ));
    }
    Ok(())
}
