use std::collections::{btree_map::Entry, BTreeMap};

use rusqlite::OptionalExtension as _;

use crate::control_store::model::{MAX_CONTROL_EFFECTS, MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES};

use super::*;

pub(in crate::control_store::aggregate) fn read_effects_from(
    connection: &Connection,
    installation: &InstallationId,
    operation_id: &str,
) -> UseResult<Vec<ControlEffectRecord>> {
    validate_effect_inventory(connection, operation_id)?;
    let mut statement = connection
        .prepare(
            "SELECT c.sequence, o.idempotency_key, c.installation_generation,
                    c.subject_kind, c.package_id, c.package_lifecycle_generation,
                    c.surface_kind, c.surface_id, o.provider_id, c.checkpoint_kind,
                    o.payload_json, o.payload_digest, c.required, o.status, o.attempt,
                    o.claim_owner, o.claim_token, o.lease_until_ms, o.evidence_digest,
                    o.error_code, o.observed_at_ms, i.scope_kind, i.scope_id,
                    p.plan_digest, p.action, p.expected_generation,
                    p.expected_capability_generation
             FROM lifecycle_checkpoint c
             JOIN effect_outbox o
               ON o.operation_id = c.operation_id AND o.sequence = c.sequence
             JOIN control_operation p ON p.operation_id = c.operation_id
             JOIN control_installation i ON i.singleton = 1
             WHERE c.operation_id = ?1 ORDER BY c.sequence",
        )
        .map_err(|error| schema::sqlite_error("prepare Control Store effects read", error))?;
    let rows = statement
        .query_map([operation_id], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<String>>(6)?,
                row.get::<_, Option<String>>(7)?,
                row.get::<_, String>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, Vec<u8>>(10)?,
                row.get::<_, String>(11)?,
                row.get::<_, bool>(12)?,
                row.get::<_, String>(13)?,
                row.get::<_, i64>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<String>>(16)?,
                row.get::<_, Option<i64>>(17)?,
                row.get::<_, Option<String>>(18)?,
                row.get::<_, Option<String>>(19)?,
                row.get::<_, Option<i64>>(20)?,
                row.get::<_, String>(21)?,
                row.get::<_, String>(22)?,
                row.get::<_, String>(23)?,
                row.get::<_, String>(24)?,
                row.get::<_, i64>(25)?,
                row.get::<_, i64>(26)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query Control Store effects", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store effects", error))?;
    let mut generations = BTreeMap::new();
    rows.into_iter()
        .enumerate()
        .map(|(index, row)| {
            let sequence = u32::try_from(row.0)
                .map_err(|_| corruption_error("A Control Store effect sequence is invalid."))?;
            let attempt = u32::try_from(row.14)
                .map_err(|_| corruption_error("A Control Store effect attempt is invalid."))?;
            let installation_generation = from_i64(row.2)?;
            let package_lifecycle_generation = row.5.map(from_i64).transpose()?;
            let kind = ControlEffectKind::parse(&row.9)?;
            let operation_action = parse_operation_action(&row.24)?;
            let expected_generation = from_i64(row.25)?;
            let expected_capability_generation = from_i64(row.26)?;
            let target_generation = expected_generation
                .checked_add(1)
                .ok_or_else(|| corruption_error("A Control Store generation is exhausted."))?;
            let is_target = installation_generation == target_generation;
            let intent = serde_json::from_slice::<ControlEffectIntent>(&row.10).map_err(|_| {
                corruption_error("A Control Store effect payload is not valid typed JSON.")
            })?;
            let canonical = intent.canonical_bytes().map_err(|_| {
                corruption_error("A Control Store effect payload is not canonically encodable.")
            })?;
            let payload_digest = intent.descriptor_digest().map_err(|_| {
                corruption_error("A Control Store effect payload digest cannot be derived.")
            })?;
            if usize::try_from(sequence).ok() != Some(index)
                || intent.sequence != sequence
                || intent.idempotency_key != row.1
                || intent.installation.kind.as_str() != row.21
                || intent.installation.id != row.22
                || intent.plan_digest != row.23
                || intent.installation_generation != installation_generation
                || intent.provider_id != row.8
                || intent.kind != kind
                || intent.required != row.12
                || intent.subject.kind_name() != row.3
                || !subject_projection_matches(
                    &intent.subject,
                    row.4.as_deref(),
                    package_lifecycle_generation,
                    row.6.as_deref(),
                    row.7.as_deref(),
                )
                || canonical != row.10
                || payload_digest != row.11
                || (!is_target
                    && (expected_generation == 0
                        || installation_generation != expected_generation))
            {
                return Err(corruption_error(
                    "A Control Store effect payload drifted from its relational identity or digest.",
                ));
            }
            intent
                .validate_binding(installation, &row.23, operation_action)
                .map_err(|_| {
                    corruption_error(
                        "A Control Store effect payload does not bind its reviewed operation.",
                    )
                })?;
            if matches!(
                &intent.subject,
                ControlEffectSubject::Installation {
                    expected_capability_generation: expected,
                    ..
                } if *expected != expected_capability_generation
            ) {
                return Err(corruption_error(
                    "A Control Store graph effect does not bind the reviewed capability cursor.",
                ));
            }
            let generation = match generations.entry(installation_generation) {
                Entry::Occupied(entry) => entry.into_mut(),
                Entry::Vacant(entry) => {
                    let generation = read_generation_from(
                        connection,
                        installation,
                        installation_generation,
                    )?
                    .ok_or_else(|| {
                        corruption_error(
                            "A Control Store effect references a missing installation generation.",
                        )
                    })?;
                    entry.insert(generation)
                }
            };
            if !intent.subject.matches_generation(
                &generation.snapshot,
                &generation.package_lifecycles,
                &generation.capability,
                is_target,
                operation_action,
            ) {
                return Err(corruption_error(
                    "A Control Store effect payload does not bind its committed generation.",
                ));
            }
            Ok(ControlEffectRecord {
                operation_id: operation_id.to_string(),
                intent,
                payload_digest: row.11,
                status: ControlEffectStatus::parse(&row.13)?,
                attempt,
                claim_owner: row.15,
                claim_token: row.16,
                lease_until_ms: row.17.map(from_i64).transpose()?,
                evidence_digest: row.18,
                error_code: row.19,
                observed_at_ms: row.20.map(from_i64).transpose()?,
            })
        })
        .collect()
}

fn validate_effect_inventory(connection: &Connection, operation_id: &str) -> UseResult<()> {
    let (checkpoint_count, outbox_count, payload_bytes): (i64, i64, i64) = connection
        .query_row(
            "SELECT
                (SELECT COUNT(*) FROM lifecycle_checkpoint WHERE operation_id = ?1),
                (SELECT COUNT(*) FROM effect_outbox WHERE operation_id = ?1),
                COALESCE((SELECT SUM(length(payload_json)) FROM effect_outbox
                          WHERE operation_id = ?1), 0)",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| schema::sqlite_error("bound Control Store effect inventory", error))?;
    let checkpoint_count = usize::try_from(checkpoint_count)
        .map_err(|_| corruption_error("The Control Store effect count is invalid."))?;
    let outbox_count = usize::try_from(outbox_count)
        .map_err(|_| corruption_error("The Control Store outbox count is invalid."))?;
    let payload_bytes = usize::try_from(payload_bytes)
        .map_err(|_| corruption_error("The Control Store effect payload size is invalid."))?;
    if checkpoint_count != outbox_count
        || outbox_count > MAX_CONTROL_EFFECTS
        || payload_bytes > MAX_CONTROL_EFFECT_PAYLOAD_TOTAL_BYTES
    {
        return Err(corruption_error(
            "The Control Store effect inventory is incomplete or exceeds its bounds.",
        ));
    }
    Ok(())
}

fn subject_projection_matches(
    subject: &ControlEffectSubject,
    package_id: Option<&str>,
    lifecycle_generation: Option<u64>,
    surface_kind: Option<&str>,
    surface_id: Option<&str>,
) -> bool {
    match subject {
        ControlEffectSubject::Installation { .. } => {
            package_id.is_none()
                && lifecycle_generation.is_none()
                && surface_kind.is_none()
                && surface_id.is_none()
        }
        ControlEffectSubject::Package {
            package_id: expected_package,
            lifecycle_generation: expected_generation,
            ..
        } => {
            package_id == Some(expected_package.as_str())
                && lifecycle_generation == Some(*expected_generation)
                && surface_kind.is_none()
                && surface_id.is_none()
        }
        ControlEffectSubject::Surface {
            package_id: expected_package,
            lifecycle_generation: expected_generation,
            surface,
            ..
        } => {
            package_id == Some(expected_package.as_str())
                && lifecycle_generation == Some(*expected_generation)
                && surface_kind == Some(surface_kind_name(surface.kind))
                && surface_id == Some(surface.id.as_str())
        }
    }
}

pub(in crate::control_store::aggregate) fn read_next_unfinished_effect(
    connection: &Connection,
    installation: &InstallationId,
    operation_id: &str,
) -> UseResult<Option<ControlEffectRecord>> {
    let effects = read_effects_from(connection, installation, operation_id)?;
    Ok(effects.into_iter().find(|effect| {
        !matches!(
            effect.status,
            ControlEffectStatus::Applied | ControlEffectStatus::Rejected
        )
    }))
}

pub(in crate::control_store::aggregate) fn read_effect_by_key(
    connection: &Connection,
    installation: &InstallationId,
    idempotency_key: &str,
) -> UseResult<Option<ControlEffectRecord>> {
    let operation_id = connection
        .query_row(
            "SELECT operation_id FROM effect_outbox WHERE idempotency_key = ?1",
            [idempotency_key],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(|error| schema::sqlite_error("locate Control Store effect", error))?;
    let Some(operation_id) = operation_id else {
        return Ok(None);
    };
    Ok(read_effects_from(connection, installation, &operation_id)?
        .into_iter()
        .find(|effect| effect.intent.idempotency_key == idempotency_key))
}
