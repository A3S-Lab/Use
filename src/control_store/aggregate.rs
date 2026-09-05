use std::path::Path;

use a3s_use_core::{
    InstallationId, InstallationSnapshot, LockedPluginPackage, PlanQualifiedSurfaceRef,
    PlannedProviderEvidence, PluginSurfaceRef, PluginWorkspaceGrant, UseError, UseResult,
};
use olpc_cjson::CanonicalFormatter;
use rusqlite::{params, Connection, ErrorCode, Row, Transaction, TransactionBehavior};
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::export::ControlStoreExport;
use super::model::{
    conflict_error, corruption_error, enforcement_profile_name, input_error, operation_action_name,
    parse_enforcement_profile, parse_operation_action, parse_surface_kind, surface_kind_name,
    valid_error_code, valid_machine_id, valid_sha256, validate_grant_selections,
    validate_provider_selections, ClaimedControlEffect, ControlAppliedEffect,
    ControlAppliedEffectEvidence, ControlAuthorizationEvidence, ControlCapabilitySelection,
    ControlCapabilityStatus, ControlEffectClaim, ControlEffectIntent, ControlEffectKind,
    ControlEffectObservation, ControlEffectOutcome, ControlEffectOwner, ControlEffectRecord,
    ControlEffectStatus, ControlEffectSubject, ControlGeneration, ControlGrantSelection,
    ControlOperationRecord, ControlOperationStatus, ControlPackageLifecycle,
    ControlProjectionHistory, ControlProviderSelection, ControlPublishedCapabilityCursor,
    ControlStoreAuthority, ControlTransition, ReviewedControlOperation,
    MAX_CONTROL_HISTORY_PACKAGES,
};
use super::schema;

mod effect_authority;
mod generation;
mod read;
mod restore;

use generation::{insert_generation, read_projection_history};
use read::*;
pub(super) use restore::restore_export;

pub(super) fn register_operation(
    path: &Path,
    installation: &InstallationId,
    reviewed: &ReviewedControlOperation,
) -> UseResult<ControlOperationRecord> {
    reviewed.validate_for_installation(installation)?;
    let plan_json = reviewed.canonical_plan_bytes()?;
    let authorization_json = reviewed.authorization.canonical_bytes()?;
    let authorization_digest = reviewed.authorization_digest()?;
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "register reviewed operation")?;
    if let Some(existing) =
        read_operation_from(&transaction, installation, reviewed.operation_id())?
    {
        if existing.reviewed == *reviewed {
            transaction
                .commit()
                .map_err(|error| schema::sqlite_error("finish reviewed operation replay", error))?;
            return Ok(existing);
        }
        return Err(conflict_error(
            "The Control Store operation ID already binds different reviewed evidence.",
        ));
    }
    let (generation, capability_generation) = read_cursors(&transaction)?;
    if generation != reviewed.expected_generation
        || capability_generation != reviewed.expected_capability_generation
    {
        return Err(generation_changed());
    }
    transaction
        .execute(
            "INSERT INTO control_operation(
                operation_id, plan_json, plan_digest,
                authorization_json, authorization_digest, action, root_package_id,
                expected_generation, target_generation,
                expected_capability_generation, target_capability_generation,
                reviewed_at_ms, status
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, 'reviewed')",
            params![
                reviewed.operation_id(),
                plan_json,
                reviewed.plan_digest(),
                authorization_json,
                authorization_digest,
                operation_action_name(reviewed.action()),
                reviewed.root_package_id(),
                to_i64(reviewed.expected_generation)?,
                to_i64(reviewed.target_generation()?)?,
                to_i64(reviewed.expected_capability_generation)?,
                to_i64(reviewed.target_capability_generation()?)?,
                to_i64(reviewed.reviewed_at_ms)?,
            ],
        )
        .map_err(|error| mutation_error("register reviewed Control Store operation", error))?;
    let record = read_operation_from(&transaction, installation, reviewed.operation_id())?
        .ok_or_else(|| {
            corruption_error("The registered Control Store operation could not be read back.")
        })?;
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("commit reviewed Control Store operation", error))?;
    Ok(record)
}

pub(super) fn cancel_operation(
    path: &Path,
    installation: &InstallationId,
    operation_id: &str,
    plan_digest: &str,
    result_digest: &str,
    cancelled_at_ms: u64,
) -> UseResult<ControlOperationRecord> {
    validate_terminal_request(operation_id, plan_digest, result_digest, cancelled_at_ms)?;
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "cancel reviewed operation")?;
    let current = read_operation_from(&transaction, installation, operation_id)?
        .ok_or_else(|| operation_missing(operation_id))?;
    if current.reviewed.plan_digest() != plan_digest {
        return Err(conflict_error(
            "The Control Store cancellation does not match the reviewed plan.",
        ));
    }
    if current.status == ControlOperationStatus::Cancelled {
        if current.completed_at_ms == Some(cancelled_at_ms)
            && current.result_digest.as_deref() == Some(result_digest)
        {
            transaction.commit().map_err(|error| {
                schema::sqlite_error("finish Control Store cancellation replay", error)
            })?;
            return Ok(current);
        }
        return Err(conflict_error(
            "The Control Store cancellation was replayed with different evidence.",
        ));
    }
    if current.status != ControlOperationStatus::Reviewed
        || cancelled_at_ms < current.reviewed.reviewed_at_ms
    {
        return Err(conflict_error(
            "Only an exact reviewed Control Store operation can be cancelled.",
        ));
    }
    transaction
        .execute(
            "UPDATE control_operation
             SET status = 'cancelled', completed_at_ms = ?2, result_digest = ?3
             WHERE operation_id = ?1 AND status = 'reviewed'",
            params![operation_id, to_i64(cancelled_at_ms)?, result_digest],
        )
        .map_err(|error| mutation_error("cancel reviewed Control Store operation", error))?;
    let record =
        read_operation_from(&transaction, installation, operation_id)?.ok_or_else(|| {
            corruption_error("The cancelled Control Store operation could not be read back.")
        })?;
    transaction.commit().map_err(|error| {
        schema::sqlite_error("commit reviewed Control Store cancellation", error)
    })?;
    Ok(record)
}

pub(super) fn commit_transition(
    path: &Path,
    installation: &InstallationId,
    transition: &ControlTransition,
) -> UseResult<ControlGeneration> {
    let mut connection = schema::open_verified_write(path, installation)?;
    let transaction = immediate(&mut connection, "commit control transition")?;
    let operation = read_operation_from(&transaction, installation, &transition.operation_id)?
        .ok_or_else(|| operation_missing(&transition.operation_id))?;
    transition.validate(installation, &operation.reviewed)?;
    let prior = if operation.reviewed.expected_generation == 0 {
        None
    } else {
        Some(
            read_generation_from(
                &transaction,
                installation,
                operation.reviewed.expected_generation,
            )?
            .ok_or_else(|| corruption_error("The prior Control Store generation is missing."))?,
        )
    };
    let projection_history =
        read_projection_history(&transaction, operation.reviewed.expected_generation)?;
    transition.validate_projection(&operation.reviewed, prior.as_ref(), &projection_history)?;
    operation.reviewed.validate_snapshot_transition(
        prior.as_ref().map(|generation| &generation.snapshot),
        &transition.snapshot,
    )?;
    transition.validate_effect_references(prior.as_ref())?;
    if operation.status != ControlOperationStatus::Reviewed {
        if matches!(
            operation.status,
            ControlOperationStatus::EffectsPending
                | ControlOperationStatus::Completed
                | ControlOperationStatus::Rejected
        ) {
            let existing = read_generation_from(
                &transaction,
                installation,
                operation.reviewed.target_generation()?,
            )?
            .ok_or_else(|| {
                corruption_error("A committed Control Store operation is missing its generation.")
            })?;
            let effects = read_effects_from(&transaction, installation, &transition.operation_id)?;
            if generation_matches_transition(&existing, transition)
                && effects
                    .iter()
                    .map(|effect| &effect.intent)
                    .eq(transition.effects.iter())
            {
                transaction.commit().map_err(|error| {
                    schema::sqlite_error("finish Control Store transition replay", error)
                })?;
                return Ok(existing);
            }
        }
        return Err(conflict_error(
            "The Control Store operation cannot commit a different or terminal transition.",
        ));
    }
    let (generation, capability_generation) = read_cursors(&transaction)?;
    if generation != operation.reviewed.expected_generation
        || capability_generation != operation.reviewed.expected_capability_generation
    {
        return Err(generation_changed());
    }
    if transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM control_operation
                WHERE status = 'effects-pending' AND operation_id <> ?1
             )",
            [&transition.operation_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(|error| schema::sqlite_error("inspect active Control Store operation", error))?
    {
        return Err(conflict_error(
            "Another Control Store transition still owns external-effect reconciliation.",
        ));
    }
    insert_generation(&transaction, transition)?;
    let changed = transaction
        .execute(
            "UPDATE control_installation
             SET current_generation = ?3
             WHERE singleton = 1
               AND current_generation = ?1
               AND published_capability_generation = ?2",
            params![
                to_i64(operation.reviewed.expected_generation)?,
                to_i64(operation.reviewed.expected_capability_generation)?,
                to_i64(operation.reviewed.target_generation()?)?,
            ],
        )
        .map_err(|error| mutation_error("advance Control Store generation", error))?;
    if changed != 1 {
        return Err(generation_changed());
    }
    transaction
        .execute(
            "UPDATE control_operation
             SET status = 'effects-pending', committed_at_ms = ?2
             WHERE operation_id = ?1 AND status = 'reviewed'",
            params![transition.operation_id, to_i64(transition.committed_at_ms)?],
        )
        .map_err(|error| mutation_error("admit Control Store effects", error))?;
    let generation = read_generation_from(
        &transaction,
        installation,
        operation.reviewed.target_generation()?,
    )?
    .ok_or_else(|| corruption_error("The committed Control Store generation is missing."))?;
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("commit Control Store transition", error))?;
    Ok(generation)
}

/// Project the next Control transition from the reviewed operation and the
/// current durable cursors without accepting caller-supplied graph, Grant, or
/// effect fields.
///
/// This is the first half of the production composition boundary.  The read
/// transaction gives the caller one coherent operation/prior-generation
/// snapshot; the later commit still performs its own compare-and-swap and
/// projection validation because another shared reader may have raced the
/// projection.  Returning the reviewed operation alongside the transition
/// lets an external payload coordinator bind immutable Runtime plans to the
/// exact reviewed Grant proposals before publishing any bytes.
pub(super) fn project_transition(
    path: &Path,
    installation: &InstallationId,
    operation_id: &str,
    committed_at_ms: u64,
) -> UseResult<(ReviewedControlOperation, ControlTransition)> {
    if !valid_machine_id(operation_id) || committed_at_ms == 0 {
        return Err(input_error(
            "The Control Store transition projection request is invalid.",
        ));
    }
    let connection = schema::open_verified_read(path, installation)?;
    let transaction = connection.unchecked_transaction().map_err(|error| {
        schema::sqlite_error("begin Control Store transition projection", error)
    })?;
    let operation = read_operation_from(&transaction, installation, operation_id)?
        .ok_or_else(|| operation_missing(operation_id))?;
    if operation.status != ControlOperationStatus::Reviewed {
        return Err(conflict_error(
            "Only a reviewed Control Store operation can be projected for commit.",
        ));
    }
    let (generation, capability_generation) = read_cursors(&transaction)?;
    if generation != operation.reviewed.expected_generation
        || capability_generation != operation.reviewed.expected_capability_generation
    {
        return Err(generation_changed());
    }
    let prior = if operation.reviewed.expected_generation == 0 {
        None
    } else {
        Some(
            read_generation_from(
                &transaction,
                installation,
                operation.reviewed.expected_generation,
            )?
            .ok_or_else(|| corruption_error("The prior Control Store generation is missing."))?,
        )
    };
    let history = read_projection_history(&transaction, operation.reviewed.expected_generation)?;
    let projected =
        operation
            .reviewed
            .project_generation(prior.as_ref(), &history, committed_at_ms)?;
    let transition = ControlTransition {
        operation_id: operation.reviewed.operation_id().to_owned(),
        plan_digest: operation.reviewed.plan_digest().to_owned(),
        snapshot: projected.snapshot,
        package_lifecycles: projected.package_lifecycles,
        grants: projected.grants,
        provider_selections: projected.provider_selections,
        capability: projected.capability,
        effects: projected.effects,
        committed_at_ms,
    };
    transaction.commit().map_err(|error| {
        schema::sqlite_error("finish Control Store transition projection", error)
    })?;
    Ok((operation.reviewed, transition))
}

pub(super) fn operation(
    path: &Path,
    installation: &InstallationId,
    operation_id: &str,
) -> UseResult<Option<ControlOperationRecord>> {
    if !valid_machine_id(operation_id) {
        return Err(input_error("The Control Store operation ID is invalid."));
    }
    let connection = schema::open_verified_read(path, installation)?;
    read_operation_from(&connection, installation, operation_id)
}

pub(super) fn current_generation(
    path: &Path,
    installation: &InstallationId,
) -> UseResult<Option<ControlGeneration>> {
    let connection = schema::open_verified_read(path, installation)?;
    let (generation, _) = read_cursors(&connection)?;
    if generation == 0 {
        return Ok(None);
    }
    read_generation_from(&connection, installation, generation)
}

pub(super) fn published_capability(
    path: &Path,
    installation: &InstallationId,
) -> UseResult<Option<ControlPublishedCapabilityCursor>> {
    let connection = schema::open_verified_read(path, installation)?;
    let transaction = connection.unchecked_transaction().map_err(|error| {
        schema::sqlite_error(
            "begin consistent published Control capability snapshot",
            error,
        )
    })?;
    let cursor = read_published_capability_from(&transaction, installation)?;
    transaction.commit().map_err(|error| {
        schema::sqlite_error("finish published Control capability snapshot", error)
    })?;
    Ok(cursor)
}

fn read_published_capability_from(
    connection: &Connection,
    installation: &InstallationId,
) -> UseResult<Option<ControlPublishedCapabilityCursor>> {
    use rusqlite::OptionalExtension as _;

    let (_, capability_generation) = read_cursors(connection)?;
    if capability_generation == 0 {
        return Ok(None);
    }
    let installation_generation: i64 = connection
        .query_row(
            "SELECT installation_generation FROM capability_generation
             WHERE capability_generation = ?1 AND publication_state = 'published'",
            [to_i64(capability_generation)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| {
            schema::sqlite_error("read published Control capability generation", error)
        })?
        .ok_or_else(|| {
            corruption_error("The published Control capability cursor has no generation row.")
        })?;
    let generation =
        read_generation_from(connection, installation, from_i64(installation_generation)?)?
            .ok_or_else(|| {
                corruption_error("The published Control capability has no installation generation.")
            })?;
    if generation.capability.generation != capability_generation
        || generation.capability_status != ControlCapabilityStatus::Published
    {
        return Err(corruption_error(
            "The published Control capability cursor disagrees with its generation.",
        ));
    }

    let mut publication = None;
    for effect in read_effects_from(connection, installation, &generation.operation_id)? {
        if !matches!(effect.intent.owner, ControlEffectOwner::CapabilityIndex) {
            continue;
        }
        let ControlEffectSubject::Installation {
            capability_generation: effect_generation,
            descriptor_digest,
            ..
        } = &effect.intent.subject
        else {
            return Err(corruption_error(
                "A published Capability Index effect has no installation subject.",
            ));
        };
        if *effect_generation != capability_generation
            || descriptor_digest != &generation.capability.descriptor_digest
            || effect.status != ControlEffectStatus::Applied
        {
            return Err(corruption_error(
                "The published Capability Index effect does not match its generation.",
            ));
        }
        let application = effect.application.as_ref().ok_or_else(|| {
            corruption_error("A published Capability Index effect has no application evidence.")
        })?;
        application.validate_for(&effect.intent).map_err(|_| {
            corruption_error("Published Capability Index application evidence is invalid.")
        })?;
        let ControlAppliedEffectEvidence::CapabilityIndex {
            catalog,
            receipt_digest,
            ..
        } = &application.evidence
        else {
            return Err(corruption_error(
                "A published Capability Index effect has another owner's evidence.",
            ));
        };
        if publication
            .replace((receipt_digest.clone(), catalog.clone()))
            .is_some()
        {
            return Err(corruption_error(
                "A published Control capability has more than one Index receipt.",
            ));
        }
    }
    let (receipt, catalog) = publication.ok_or_else(|| {
        corruption_error("The published Control capability has no applied Index receipt.")
    })?;
    ControlPublishedCapabilityCursor::from_generation(&generation, receipt, catalog)
        .map(Some)
        .map_err(|_| corruption_error("The published Control capability cursor is invalid."))
}

pub(super) fn effects(
    path: &Path,
    installation: &InstallationId,
    operation_id: &str,
) -> UseResult<Vec<ControlEffectRecord>> {
    if !valid_machine_id(operation_id) {
        return Err(input_error("The Control Store operation ID is invalid."));
    }
    let connection = schema::open_verified_read(path, installation)?;
    read_effects_from(&connection, installation, operation_id)
}

pub(super) fn export_snapshot(
    path: &Path,
    installation: &InstallationId,
) -> UseResult<(schema::ControlStoreMetadata, ControlStoreAuthority)> {
    let connection = schema::open_verified_read(path, installation)?;
    let transaction = connection.unchecked_transaction().map_err(|error| {
        schema::sqlite_error("begin consistent Control Store export snapshot", error)
    })?;
    let (current_generation, published_capability_generation) = read_cursors(&transaction)?;
    let authority = authority_from(&transaction, installation)?;
    transaction
        .commit()
        .map_err(|error| schema::sqlite_error("finish Control Store export snapshot", error))?;
    Ok((
        schema::ControlStoreMetadata {
            installation: installation.clone(),
            schema_version: schema::CONTROL_STORE_SCHEMA_VERSION,
            current_generation,
            published_capability_generation,
        },
        authority,
    ))
}

fn authority_from(
    connection: &Connection,
    installation: &InstallationId,
) -> UseResult<ControlStoreAuthority> {
    let generation_ids = query_i64_values(
        connection,
        "SELECT generation FROM control_generation ORDER BY generation",
        "read Control Store generation inventory",
    )?;
    let generations = generation_ids
        .into_iter()
        .map(|generation| {
            let generation = from_i64(generation)?;
            read_generation_from(connection, installation, generation)?.ok_or_else(|| {
                corruption_error("A Control Store generation disappeared during export.")
            })
        })
        .collect::<UseResult<Vec<_>>>()?;

    let mut statement = connection
        .prepare("SELECT operation_id FROM control_operation ORDER BY operation_id")
        .map_err(|error| {
            schema::sqlite_error("prepare Control Store operation inventory", error)
        })?;
    let operation_ids = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| schema::sqlite_error("query Control Store operation inventory", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store operation inventory", error))?;
    let operations = operation_ids
        .iter()
        .map(|operation_id| {
            read_operation_from(connection, installation, operation_id)?.ok_or_else(|| {
                corruption_error("A Control Store operation disappeared during export.")
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    let effects = operation_ids
        .iter()
        .map(|operation_id| read_effects_from(connection, installation, operation_id))
        .collect::<UseResult<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    Ok(ControlStoreAuthority {
        generations,
        operations,
        effects,
    })
}

pub(super) fn claim_next_effect(
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
        effect_authority::derive_claim_authority(&transaction, installation, &operation, &effect)?;
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

pub(super) fn record_effect_observation(
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

pub(super) fn complete_operation(
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

fn generation_matches_transition(
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

pub(super) fn validate_operation_record(record: &ControlOperationRecord) -> UseResult<()> {
    record.reviewed.validate()?;
    if record
        .result_digest
        .as_deref()
        .is_some_and(|digest| !valid_sha256(digest))
    {
        return Err(corruption_error(
            "A Control Store operation result digest is invalid.",
        ));
    }
    let valid = match record.status {
        ControlOperationStatus::Reviewed => {
            record.committed_at_ms.is_none()
                && record.completed_at_ms.is_none()
                && record.result_digest.is_none()
        }
        ControlOperationStatus::EffectsPending => {
            record
                .committed_at_ms
                .is_some_and(|time| time >= record.reviewed.reviewed_at_ms)
                && record.completed_at_ms.is_none()
                && record.result_digest.is_none()
        }
        ControlOperationStatus::Completed | ControlOperationStatus::Rejected => {
            record
                .committed_at_ms
                .zip(record.completed_at_ms)
                .is_some_and(|(committed, completed)| {
                    committed >= record.reviewed.reviewed_at_ms && completed >= committed
                })
                && record.result_digest.is_some()
        }
        ControlOperationStatus::Cancelled => {
            record.committed_at_ms.is_none()
                && record
                    .completed_at_ms
                    .is_some_and(|time| time >= record.reviewed.reviewed_at_ms)
                && record.result_digest.is_some()
        }
    };
    if !valid {
        return Err(corruption_error(
            "A Control Store operation status does not match its timestamps and evidence.",
        ));
    }
    Ok(())
}

fn validate_terminal_request(
    operation_id: &str,
    plan_digest: &str,
    result_digest: &str,
    completed_at_ms: u64,
) -> UseResult<()> {
    if !valid_machine_id(operation_id)
        || !valid_sha256(plan_digest)
        || !valid_sha256(result_digest)
        || completed_at_ms == 0
    {
        return Err(input_error(
            "The terminal Control Store operation evidence is invalid.",
        ));
    }
    Ok(())
}

fn immediate<'a>(connection: &'a mut Connection, action: &str) -> UseResult<Transaction<'a>> {
    connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|error| schema::sqlite_error(action, error))
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        input_error(format!(
            "Failed to encode canonical Control Store value: {error}"
        ))
    })?;
    Ok(bytes)
}

fn sha256_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn to_i64(value: u64) -> UseResult<i64> {
    i64::try_from(value).map_err(|_| input_error("A Control Store integer exceeds SQLite bounds."))
}

fn from_i64(value: i64) -> UseResult<u64> {
    u64::try_from(value).map_err(|_| corruption_error("A Control Store integer is invalid."))
}

fn optional_u64(row: &Row<'_>, index: usize) -> UseResult<Option<u64>> {
    row.get::<_, Option<i64>>(index)
        .map_err(|error| schema::sqlite_error("decode optional Control Store integer", error))?
        .map(from_i64)
        .transpose()
}

fn mutation_error(action: &str, error: rusqlite::Error) -> UseError {
    if matches!(
        error,
        rusqlite::Error::SqliteFailure(
            rusqlite::ffi::Error {
                code: ErrorCode::ConstraintViolation,
                ..
            },
            _
        )
    ) {
        return conflict_error(format!(
            "The Control Store rejected {action} because an aggregate constraint changed."
        ));
    }
    schema::sqlite_error(action, error)
}

fn generation_changed() -> UseError {
    UseError::new(
        "use.control_store.generation_changed",
        "The Control Store installation or capability generation changed before commit.",
    )
}

fn operation_missing(operation_id: &str) -> UseError {
    conflict_error(format!(
        "Control Store operation '{operation_id}' does not exist."
    ))
}
