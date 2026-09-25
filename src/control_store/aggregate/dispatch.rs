//! Effect claim, observation, and completion mutations for the Control Store.

use super::*;
use super::effect_authority::derive_claim_authority;


pub(in crate::control_store) fn claim_next_effect(
    path: &Path,
    installation: &InstallationId,
    claim: &ControlEffectClaim,
) -> UseResult<Option<ClaimedControlEffect>> {
    claim.validate()?;
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "claim Control Store effect")?;
    let operation = read_operation_from(&transaction, installation, &claim.operation_id)?
        .ok_or_else(|| operation_missing(&claim.operation_id))?;
    if operation.status != ControlOperationStatus::EffectsPending {
        return Err(conflict_error(
            "Only an effect-pending Control Store operation can claim work.",
        ));
    }
    let Some(effect) =
        read_next_unfinished_effect(&transaction, installation, &claim.operation_id)?
    else {
        transaction
            .commit()
            .map_err(|error| schema::sqlite_error("finish empty Control Store claim", error))?;
        return Ok(None);
    };
    match effect.status {
        ControlEffectStatus::Deferred
            if effect
                .retry_not_before_ms
                .is_some_and(|not_before| claim.now_ms < not_before) =>
        {
            transaction.commit().map_err(|error| {
                schema::sqlite_error("finish deferred Control Store claim", error)
            })?;
            return Ok(None);
        }
        ControlEffectStatus::Claimed
            if effect
                .lease_until_ms
                .is_some_and(|lease| lease >= claim.now_ms) =>
        {
            transaction
                .commit()
                .map_err(|error| schema::sqlite_error("finish busy Control Store claim", error))?;
            return Ok(None);
        }
        ControlEffectStatus::Claimed
        | ControlEffectStatus::Unknown
        | ControlEffectStatus::Rejected
            if !claim.explicit_reconciliation =>
        {
            return Err(UseError::new(
                "use.control_store.reconciliation_required",
                "A claimed, ambiguous, or post-cutover rejected Control Store effect requires explicit reconciliation before replay.",
            ));
        }
        ControlEffectStatus::Pending
        | ControlEffectStatus::Deferred
        | ControlEffectStatus::Claimed
        | ControlEffectStatus::Unknown
        | ControlEffectStatus::Rejected => {}
        ControlEffectStatus::Applied => {
            return Err(corruption_error(
                "A terminal Control Store effect remained in the unfinished sequence.",
            ))
        }
    }
    let attempt = effect
        .attempt
        .checked_add(1)
        .ok_or_else(|| conflict_error("The Control Store effect attempt count is exhausted."))?;
    let authority =
        derive_claim_authority(&transaction, installation, &operation, &effect)?;
    transaction
        .execute(
            "UPDATE effect_outbox
             SET status = 'claimed', attempt = ?2, claim_owner = ?3,
                 claim_token = ?4, lease_until_ms = ?5,
                 application_json = NULL, evidence_digest = NULL,
                 error_code = NULL, observed_at_ms = NULL,
                 retry_not_before_ms = NULL
             WHERE idempotency_key = ?1",
            params![
                effect.intent.idempotency_key,
                i64::from(attempt),
                claim.worker_id,
                claim.claim_token,
                to_i64(claim.lease_until_ms)?,
            ],
        )
        .map_err(|error| mutation_error("claim Control Store outbox effect", error))?;
    let claimed = ClaimedControlEffect {
        intent: effect.intent,
        authority,
        attempt,
        claim_token: claim.claim_token.clone(),
        lease_until_ms: claim.lease_until_ms,
    };
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("commit Control Store effect claim", error))?;
    Ok(Some(claimed))
}

pub(in crate::control_store) fn record_effect_observation(
    path: &Path,
    installation: &InstallationId,
    observation: &ControlEffectObservation,
) -> UseResult<bool> {
    observation.validate()?;
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "record Control Store effect")?;
    let current = read_effect_by_key(&transaction, installation, &observation.idempotency_key)?
        .ok_or_else(|| conflict_error("The Control Store effect does not exist."))?;
    if current.operation_id != observation.operation_id {
        return Err(conflict_error(
            "The Control Store effect belongs to a different operation.",
        ));
    }
    let committed_at_ms =
        read_operation_from(&transaction, installation, &observation.operation_id)?
            .and_then(|operation| operation.committed_at_ms)
            .ok_or_else(|| {
                corruption_error("A Control Store effect has no committed operation transition.")
            })?;
    let prior_observed_at_ms = transaction
        .query_row(
            "SELECT MAX(observed_at_ms) FROM effect_outbox
             WHERE operation_id = ?1 AND sequence < ?2",
            params![observation.operation_id, i64::from(current.intent.sequence)],
            |row| row.get::<_, Option<i64>>(0),
        )
        .map_err(|error| {
            schema::sqlite_error("inspect prior Control Store effect observations", error)
        })?
        .map(from_i64)
        .transpose()?;
    let earliest_observation_ms = prior_observed_at_ms
        .map(|observed_at_ms| observed_at_ms.max(committed_at_ms))
        .unwrap_or(committed_at_ms);
    if observation.observed_at_ms < earliest_observation_ms {
        return Err(conflict_error(
            "The Control Store effect observation predates its transition or prior observation.",
        ));
    }
    let (application_json, evidence_digest) = observation.evidence_for(&current.intent)?;
    if matches!(
        current.status,
        ControlEffectStatus::Deferred
            | ControlEffectStatus::Applied
            | ControlEffectStatus::Rejected
            | ControlEffectStatus::Unknown
    ) {
        if current.status == observation.outcome.status()
            && current.claim_token.as_deref() == Some(&observation.claim_token)
            && current.application == observation.application
            && current.evidence_digest.as_deref() == Some(&evidence_digest)
            && current.error_code == observation.error_code
            && current.observed_at_ms == Some(observation.observed_at_ms)
            && current.retry_not_before_ms == observation.retry_not_before_ms
        {
            transaction.commit().map_err(|error| {
                schema::sqlite_error("finish Control Store observation replay", error)
            })?;
            return Ok(false);
        }
        return Err(conflict_error(
            "A Control Store effect observation conflicts with durable evidence.",
        ));
    }
    if current.status != ControlEffectStatus::Claimed
        || current.claim_token.as_deref() != Some(&observation.claim_token)
        || current
            .lease_until_ms
            .is_none_or(|lease| observation.observed_at_ms > lease)
    {
        return Err(conflict_error(
            "The Control Store effect observation does not own the active claim lease.",
        ));
    }
    transaction
        .execute(
            "UPDATE effect_outbox
             SET status = ?2, application_json = ?3, evidence_digest = ?4,
                 error_code = ?5, observed_at_ms = ?6, retry_not_before_ms = ?7
             WHERE idempotency_key = ?1 AND status = 'claimed'",
            params![
                observation.idempotency_key,
                observation.outcome.status().as_str(),
                application_json,
                evidence_digest,
                observation.error_code,
                to_i64(observation.observed_at_ms)?,
                observation.retry_not_before_ms.map(to_i64).transpose()?,
            ],
        )
        .map_err(|error| mutation_error("record Control Store outbox observation", error))?;
    if observation.outcome == ControlEffectOutcome::Applied
        && current.intent.kind == ControlEffectKind::CapabilityCutover
    {
        publish_capability_cutover(&transaction, &current.intent, observation.observed_at_ms)?;
    }
    if observation.outcome == ControlEffectOutcome::Rejected
        && current.intent.required
        && !capability_cutover_applied(&transaction, &observation.operation_id)?
    {
        transaction
            .execute(
                "UPDATE control_operation
                 SET status = 'rejected', completed_at_ms = ?2, result_digest = ?3
                 WHERE operation_id = ?1 AND status = 'effects-pending'",
                params![
                    observation.operation_id,
                    to_i64(observation.observed_at_ms)?,
                    evidence_digest,
                ],
            )
            .map_err(|error| mutation_error("reject Control Store operation", error))?;
        transaction
            .execute(
                "UPDATE capability_generation
                 SET publication_state = 'abandoned'
                 WHERE installation_generation = (
                    SELECT target_generation FROM control_operation WHERE operation_id = ?1
                 ) AND publication_state = 'candidate'",
                [&observation.operation_id],
            )
            .map_err(|error| {
                mutation_error(
                    "abandon rejected Control Store capability generation",
                    error,
                )
            })?;
    }
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("commit Control Store effect observation", error))?;
    Ok(true)
}

fn publish_capability_cutover(
    transaction: &Transaction<'_>,
    intent: &ControlEffectIntent,
    published_at_ms: u64,
) -> UseResult<()> {
    let ControlEffectSubject::Installation {
        expected_capability_generation,
        capability_generation,
        descriptor_digest,
    } = &intent.subject
    else {
        return Err(corruption_error(
            "A capability cutover effect has a non-installation subject.",
        ));
    };
    if *expected_capability_generation > 0 {
        let retired = transaction
            .execute(
                "UPDATE capability_generation
                 SET publication_state = 'retired'
                 WHERE capability_generation = ?1 AND publication_state = 'published'",
                [to_i64(*expected_capability_generation)?],
            )
            .map_err(|error| {
                mutation_error("retire prior Control Store capability generation", error)
            })?;
        if retired != 1 {
            return Err(conflict_error(
                "The prior published Control Store capability generation is missing.",
            ));
        }
    }
    let published = transaction
        .execute(
            "UPDATE capability_generation
             SET publication_state = 'published', published_at_ms = ?4
             WHERE capability_generation = ?1
               AND installation_generation = ?2
               AND descriptor_digest = ?3
               AND publication_state = 'candidate'",
            params![
                to_i64(*capability_generation)?,
                to_i64(intent.installation_generation)?,
                descriptor_digest,
                to_i64(published_at_ms)?,
            ],
        )
        .map_err(|error| mutation_error("publish Control Store capability generation", error))?;
    if published != 1 {
        return Err(conflict_error(
            "The Control Store capability generation changed before cutover observation.",
        ));
    }
    let advanced = transaction
        .execute(
            "UPDATE control_installation
             SET published_capability_generation = ?3
             WHERE singleton = 1 AND current_generation = ?1
               AND published_capability_generation = ?2",
            params![
                to_i64(intent.installation_generation)?,
                to_i64(*expected_capability_generation)?,
                to_i64(*capability_generation)?,
            ],
        )
        .map_err(|error| mutation_error("advance published capability generation", error))?;
    if advanced != 1 {
        return Err(generation_changed());
    }
    Ok(())
}

fn capability_cutover_applied(
    transaction: &Transaction<'_>,
    operation_id: &str,
) -> UseResult<bool> {
    transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM lifecycle_checkpoint c
                JOIN effect_outbox o
                  ON o.operation_id = c.operation_id AND o.sequence = c.sequence
                WHERE c.operation_id = ?1
                  AND c.checkpoint_kind = 'capability-cutover'
                  AND o.status = 'applied'
             )",
            [operation_id],
            |row| row.get(0),
        )
        .map_err(|error| schema::sqlite_error("inspect Control Store capability cutover", error))
}

pub(in crate::control_store) fn complete_operation(
    path: &Path,
    installation: &InstallationId,
    operation_id: &str,
    plan_digest: &str,
    result_digest: &str,
    completed_at_ms: u64,
) -> UseResult<ControlOperationRecord> {
    validate_terminal_request(operation_id, plan_digest, result_digest, completed_at_ms)?;
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "complete Control Store operation")?;
    let current = read_operation_from(&transaction, installation, operation_id)?
        .ok_or_else(|| operation_missing(operation_id))?;
    if current.reviewed.plan_digest() != plan_digest {
        return Err(conflict_error(
            "The Control Store completion does not match the reviewed plan.",
        ));
    }
    if current.status == ControlOperationStatus::Completed {
        if current.completed_at_ms == Some(completed_at_ms)
            && current.result_digest.as_deref() == Some(result_digest)
        {
            transaction.commit().map_err(|error| {
                schema::sqlite_error("finish Control Store completion replay", error)
            })?;
            return Ok(current);
        }
        return Err(conflict_error(
            "The Control Store completion was replayed with different evidence.",
        ));
    }
    if current.status != ControlOperationStatus::EffectsPending
        || completed_at_ms < current.committed_at_ms.unwrap_or(u64::MAX)
    {
        return Err(conflict_error(
            "Only an effect-pending Control Store operation can complete.",
        ));
    }
    let effects = read_effects_from(&transaction, installation, operation_id)?;
    if effects.iter().any(|effect| {
        (effect.intent.required || effect.status != ControlEffectStatus::Rejected)
            && effect.status != ControlEffectStatus::Applied
    }) {
        return Err(conflict_error(
            "The Control Store operation still has unfinished or rejected required effects.",
        ));
    }
    if effects
        .iter()
        .filter_map(|effect| effect.observed_at_ms)
        .max()
        .is_some_and(|observed_at_ms| observed_at_ms > completed_at_ms)
    {
        return Err(conflict_error(
            "The Control Store completion predates an external-effect observation.",
        ));
    }
    let target_generation = current.reviewed.target_generation()?;
    let target_capability_generation = current.reviewed.target_capability_generation()?;
    let completed_at_i64 = to_i64(completed_at_ms)?;
    let (publication_state, published_at_ms): (String, Option<i64>) = transaction
        .query_row(
            "SELECT publication_state, published_at_ms FROM capability_generation
             WHERE installation_generation = ?1 AND capability_generation = ?2",
            params![
                to_i64(target_generation)?,
                to_i64(target_capability_generation)?
            ],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| schema::sqlite_error("verify applied capability cutover", error))?;
    let (_, published_cursor) = read_cursors(&transaction)?;
    if publication_state != "published"
        || published_cursor != target_capability_generation
        || published_at_ms.is_none_or(|time| time <= 0 || time > completed_at_i64)
    {
        return Err(conflict_error(
            "The Control Store capability cutover has not been durably observed.",
        ));
    }
    transaction
        .execute(
            "UPDATE control_operation
             SET status = 'completed', completed_at_ms = ?2, result_digest = ?3
             WHERE operation_id = ?1 AND status = 'effects-pending'",
            params![operation_id, to_i64(completed_at_ms)?, result_digest],
        )
        .map_err(|error| mutation_error("complete Control Store operation", error))?;
    let completed =
        read_operation_from(&transaction, installation, operation_id)?.ok_or_else(|| {
            corruption_error("The completed Control Store operation could not be read back.")
        })?;
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("commit Control Store completion", error))?;
    Ok(completed)
}

pub(super) fn generation_matches_transition(
    generation: &ControlGeneration,
    transition: &ControlTransition,
) -> bool {
    generation.operation_id == transition.operation_id
        && generation.snapshot == transition.snapshot
        && generation.package_lifecycles == transition.package_lifecycles
        && generation.grants == transition.grants
        && generation.provider_selections == transition.provider_selections
        && generation.capability == transition.capability
        && generation.committed_at_ms == transition.committed_at_ms
}
