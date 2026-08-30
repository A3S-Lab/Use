use rusqlite::OptionalExtension as _;

use super::*;

pub(super) fn read_operation_from(
    connection: &Connection,
    operation_id: &str,
) -> UseResult<Option<ControlOperationRecord>> {
    let mut statement = connection
        .prepare(
            "SELECT plan_digest, authorization_digest, action, root_package_id,
                    expected_generation, expected_capability_generation,
                    reviewed_at_ms, status, committed_at_ms, completed_at_ms,
                    result_digest
             FROM control_operation WHERE operation_id = ?1",
        )
        .map_err(|error| schema::sqlite_error("prepare Control Store operation read", error))?;
    let mut rows = statement
        .query([operation_id])
        .map_err(|error| schema::sqlite_error("query Control Store operation", error))?;
    let Some(row) = rows
        .next()
        .map_err(|error| schema::sqlite_error("read Control Store operation", error))?
    else {
        return Ok(None);
    };
    let record = operation_from_row(row, operation_id)?;
    if rows
        .next()
        .map_err(|error| schema::sqlite_error("bound Control Store operation read", error))?
        .is_some()
    {
        return Err(corruption_error(
            "A Control Store operation appears more than once.",
        ));
    }
    Ok(Some(record))
}

fn operation_from_row(row: &Row<'_>, operation_id: &str) -> UseResult<ControlOperationRecord> {
    let action: String = row
        .get(2)
        .map_err(|error| schema::sqlite_error("decode Control Store operation action", error))?;
    let status: String = row
        .get(7)
        .map_err(|error| schema::sqlite_error("decode Control Store operation status", error))?;
    let record = ControlOperationRecord {
        reviewed: ReviewedControlOperation {
            operation_id: operation_id.to_string(),
            plan_digest: row
                .get(0)
                .map_err(|error| schema::sqlite_error("decode Control Store plan digest", error))?,
            authorization_digest: row.get(1).map_err(|error| {
                schema::sqlite_error("decode Control Store authorization digest", error)
            })?,
            action: parse_operation_action(&action)?,
            root_package_id: PluginPackageId::parse(row.get::<_, String>(3).map_err(|error| {
                schema::sqlite_error("decode Control Store root package", error)
            })?)
            .map_err(|_| corruption_error("A Control Store root package ID is invalid."))?,
            expected_generation: from_i64(row.get(4).map_err(|error| {
                schema::sqlite_error("decode Control Store expected generation", error)
            })?)?,
            expected_capability_generation: from_i64(row.get(5).map_err(|error| {
                schema::sqlite_error("decode Control Store expected capability generation", error)
            })?)?,
            reviewed_at_ms: from_i64(row.get(6).map_err(|error| {
                schema::sqlite_error("decode Control Store review time", error)
            })?)?,
        },
        status: ControlOperationStatus::parse(&status)?,
        committed_at_ms: optional_u64(row, 8)?,
        completed_at_ms: optional_u64(row, 9)?,
        result_digest: row
            .get(10)
            .map_err(|error| schema::sqlite_error("decode Control Store result digest", error))?,
    };
    validate_operation_record(&record)?;
    Ok(record)
}

pub(super) fn read_generation_from(
    connection: &Connection,
    installation: &InstallationId,
    generation: u64,
) -> UseResult<Option<ControlGeneration>> {
    let mut statement = connection
        .prepare(
            "SELECT operation_id, snapshot_json, snapshot_digest, committed_at_ms
             FROM control_generation WHERE generation = ?1",
        )
        .map_err(|error| schema::sqlite_error("prepare Control Store generation read", error))?;
    let mut rows = statement
        .query([to_i64(generation)?])
        .map_err(|error| schema::sqlite_error("query Control Store generation", error))?;
    let Some(row) = rows
        .next()
        .map_err(|error| schema::sqlite_error("read Control Store generation", error))?
    else {
        return Ok(None);
    };
    let operation_id: String = row.get(0).map_err(|error| {
        schema::sqlite_error("decode Control Store generation operation", error)
    })?;
    let snapshot_json: Vec<u8> = row
        .get(1)
        .map_err(|error| schema::sqlite_error("decode Control Store generation snapshot", error))?;
    let snapshot_digest: String = row
        .get(2)
        .map_err(|error| schema::sqlite_error("decode Control Store generation digest", error))?;
    let committed_at_ms =
        from_i64(row.get(3).map_err(|error| {
            schema::sqlite_error("decode Control Store generation time", error)
        })?)?;
    let snapshot = InstallationSnapshot::from_json(&snapshot_json)
        .map_err(|_| corruption_error("A Control Store installation snapshot is invalid."))?;
    if snapshot.installation != *installation
        || snapshot.generation != generation
        || snapshot.canonical_bytes()? != snapshot_json
        || snapshot.descriptor_digest()? != snapshot_digest
    {
        return Err(corruption_error(
            "A Control Store installation snapshot does not match its generation evidence.",
        ));
    }
    let package_lifecycles = validate_snapshot_relations(connection, &snapshot)?;
    let grants = read_grants(connection, generation)?;
    let bindings = read_bindings(connection, generation)?;
    let (capability_generation, descriptor_digest, publication_state, published_at_ms): (
        i64,
        String,
        String,
        Option<i64>,
    ) = connection
        .query_row(
            "SELECT capability_generation, descriptor_digest, publication_state,
                        published_at_ms
             FROM capability_generation WHERE installation_generation = ?1",
            [to_i64(generation)?],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .map_err(|error| schema::sqlite_error("read Control Store capability generation", error))?;
    let capability = ControlCapabilitySelection {
        generation: from_i64(capability_generation)?,
        descriptor_digest,
    };
    if capability.generation == 0
        || !valid_sha256(&capability.descriptor_digest)
        || !matches!(
            publication_state.as_str(),
            "candidate" | "published" | "retired" | "abandoned"
        )
        || (matches!(publication_state.as_str(), "candidate" | "abandoned")
            && published_at_ms.is_some())
        || (matches!(publication_state.as_str(), "published" | "retired")
            && published_at_ms.is_none_or(|time| time <= 0))
    {
        return Err(corruption_error(
            "A Control Store capability generation is invalid.",
        ));
    }
    Ok(Some(ControlGeneration {
        operation_id,
        snapshot,
        snapshot_digest,
        package_lifecycles,
        grants,
        bindings,
        capability,
        capability_status: ControlCapabilityStatus::parse(&publication_state)?,
        capability_published_at_ms: published_at_ms.map(from_i64).transpose()?,
        committed_at_ms,
    }))
}

fn validate_snapshot_relations(
    connection: &Connection,
    snapshot: &InstallationSnapshot,
) -> UseResult<Vec<ControlPackageLifecycle>> {
    let generation = to_i64(snapshot.generation)?;
    let mut statement = connection
        .prepare(
            "SELECT package_id, lifecycle_generation, state_generation, enabled,
                    package_json, package_digest
             FROM selected_package WHERE generation = ?1 ORDER BY package_id",
        )
        .map_err(|error| schema::sqlite_error("prepare selected package validation", error))?;
    let rows = statement
        .query_map([generation], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, Vec<u8>>(4)?,
                row.get::<_, String>(5)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query selected packages", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read selected packages", error))?;
    if rows.len() != snapshot.packages.len() {
        return Err(corruption_error(
            "Control Store selected-package rows do not match the snapshot.",
        ));
    }
    for (row, expected) in rows.iter().zip(&snapshot.packages) {
        let expected_json = canonical_json(&expected.package)?;
        let decoded: LockedPluginPackage = serde_json::from_slice(&row.4).map_err(|_| {
            corruption_error("A Control Store selected-package value is invalid JSON.")
        })?;
        if row.0 != expected.package_id()
            || from_i64(row.1)? == 0
            || from_i64(row.2)? != expected.state_generation
            || row.3 != expected.enabled
            || decoded != expected.package
            || row.4 != expected_json
            || row.5 != sha256_digest(&expected_json)
        {
            return Err(corruption_error(
                "A Control Store selected-package row drifted from its snapshot.",
            ));
        }
    }
    validate_roots(connection, snapshot)?;
    validate_dependencies(connection, snapshot)?;
    validate_surfaces(connection, snapshot)?;
    rows.into_iter()
        .map(|row| {
            Ok(ControlPackageLifecycle {
                package_id: row.0,
                lifecycle_generation: from_i64(row.1)?,
            })
        })
        .collect()
}

fn validate_roots(connection: &Connection, snapshot: &InstallationSnapshot) -> UseResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, installed_at_ms FROM installation_root
             WHERE generation = ?1 ORDER BY package_id",
        )
        .map_err(|error| schema::sqlite_error("prepare Control Store root validation", error))?;
    let actual = statement
        .query_map([to_i64(snapshot.generation)?], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| schema::sqlite_error("query Control Store roots", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store roots", error))?;
    let expected = snapshot
        .roots
        .iter()
        .map(|root| Ok((root.package_id.clone(), to_i64(root.installed_at_ms)?)))
        .collect::<UseResult<Vec<_>>>()?;
    if actual != expected {
        return Err(corruption_error(
            "Control Store root rows do not match the installation snapshot.",
        ));
    }
    Ok(())
}

fn validate_dependencies(
    connection: &Connection,
    snapshot: &InstallationSnapshot,
) -> UseResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, dependency_package_id, version_requirement, selected_version
             FROM package_dependency WHERE generation = ?1
             ORDER BY package_id, dependency_package_id",
        )
        .map_err(|error| schema::sqlite_error("prepare dependency validation", error))?;
    let actual = statement
        .query_map([to_i64(snapshot.generation)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query Control Store dependencies", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store dependencies", error))?;
    let expected = snapshot
        .packages
        .iter()
        .flat_map(|package| {
            package.package.dependencies.iter().map(move |dependency| {
                (
                    package.package_id().to_string(),
                    dependency.package_id.clone(),
                    dependency.version_requirement.clone(),
                    dependency.version.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    if actual != expected {
        return Err(corruption_error(
            "Control Store dependency rows do not match the installation snapshot.",
        ));
    }
    Ok(())
}

fn validate_surfaces(connection: &Connection, snapshot: &InstallationSnapshot) -> UseResult<()> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, surface_kind, surface_id FROM selected_surface
             WHERE generation = ?1 ORDER BY package_id, surface_kind, surface_id",
        )
        .map_err(|error| schema::sqlite_error("prepare surface validation", error))?;
    let actual = statement
        .query_map([to_i64(snapshot.generation)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query Control Store surfaces", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store surfaces", error))?;
    let mut expected = snapshot
        .packages
        .iter()
        .flat_map(|package| {
            package.selected_surfaces.iter().map(move |surface| {
                (
                    package.package_id().to_string(),
                    surface_kind_name(surface.kind).to_string(),
                    surface.id.clone(),
                )
            })
        })
        .collect::<Vec<_>>();
    expected.sort();
    if actual != expected {
        return Err(corruption_error(
            "Control Store surface rows do not match the installation snapshot.",
        ));
    }
    Ok(())
}

fn read_grants(connection: &Connection, generation: u64) -> UseResult<Vec<ControlGrantSelection>> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, grant_json, grant_digest FROM control_grant
             WHERE generation = ?1 ORDER BY package_id",
        )
        .map_err(|error| schema::sqlite_error("prepare Control Store Grant read", error))?;
    let rows = statement
        .query_map([to_i64(generation)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query Control Store Grants", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store Grants", error))?;
    rows.into_iter()
        .map(|(package_id, grant_json, grant_digest)| {
            let grant = PluginWorkspaceGrant::from_json(&grant_json)
                .map_err(|_| corruption_error("A Control Store Grant value is invalid."))?;
            if grant.package_id != package_id
                || grant.canonical_bytes()? != grant_json
                || grant.descriptor_digest()? != grant_digest
            {
                return Err(corruption_error(
                    "A Control Store Grant is not canonically bound to its row.",
                ));
            }
            Ok(ControlGrantSelection {
                grant,
                grant_digest,
            })
        })
        .collect()
}

fn read_bindings(
    connection: &Connection,
    generation: u64,
) -> UseResult<Vec<ControlProviderBinding>> {
    let mut statement = connection
        .prepare(
            "SELECT package_id, surface_kind, surface_id, provider_id, binding_digest
             FROM provider_binding WHERE generation = ?1
             ORDER BY package_id, surface_kind, surface_id, provider_id",
        )
        .map_err(|error| schema::sqlite_error("prepare Control Store binding read", error))?;
    let rows = statement
        .query_map([to_i64(generation)?], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query Control Store bindings", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store bindings", error))?;
    rows.into_iter()
        .map(|row| {
            if !valid_machine_id(&row.3) || !valid_sha256(&row.4) {
                return Err(corruption_error(
                    "A Control Store provider binding is invalid.",
                ));
            }
            Ok(ControlProviderBinding {
                package_id: row.0,
                surface: PluginSurfaceRef {
                    kind: parse_surface_kind(&row.1)?,
                    id: row.2,
                },
                provider_id: row.3,
                binding_digest: row.4,
            })
        })
        .collect()
}

pub(super) fn read_effects_from(
    connection: &Connection,
    operation_id: &str,
) -> UseResult<Vec<ControlEffectRecord>> {
    let mut statement = connection
        .prepare(
            "SELECT c.sequence, o.idempotency_key, c.installation_generation,
                    c.package_id, c.package_lifecycle_generation, o.provider_id,
                    c.checkpoint_kind, o.payload_digest,
                    c.required, o.status, o.attempt, o.claim_owner, o.claim_token,
                    o.lease_until_ms, o.evidence_digest, o.error_code,
                    o.observed_at_ms
             FROM lifecycle_checkpoint c
             JOIN effect_outbox o
               ON o.operation_id = c.operation_id AND o.sequence = c.sequence
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
                row.get::<_, i64>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, bool>(8)?,
                row.get::<_, String>(9)?,
                row.get::<_, i64>(10)?,
                row.get::<_, Option<String>>(11)?,
                row.get::<_, Option<String>>(12)?,
                row.get::<_, Option<i64>>(13)?,
                row.get::<_, Option<String>>(14)?,
                row.get::<_, Option<String>>(15)?,
                row.get::<_, Option<i64>>(16)?,
            ))
        })
        .map_err(|error| schema::sqlite_error("query Control Store effects", error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error("read Control Store effects", error))?;
    rows.into_iter()
        .map(|row| {
            let sequence = u32::try_from(row.0)
                .map_err(|_| corruption_error("A Control Store effect sequence is invalid."))?;
            let attempt = u32::try_from(row.10)
                .map_err(|_| corruption_error("A Control Store effect attempt is invalid."))?;
            Ok(ControlEffectRecord {
                operation_id: operation_id.to_string(),
                intent: ControlEffectIntent {
                    sequence,
                    idempotency_key: row.1,
                    installation_generation: from_i64(row.2)?,
                    package_id: row.3,
                    package_lifecycle_generation: from_i64(row.4)?,
                    provider_id: row.5,
                    kind: ControlEffectKind::parse(&row.6)?,
                    payload_digest: row.7,
                    required: row.8,
                },
                status: ControlEffectStatus::parse(&row.9)?,
                attempt,
                claim_owner: row.11,
                claim_token: row.12,
                lease_until_ms: row.13.map(from_i64).transpose()?,
                evidence_digest: row.14,
                error_code: row.15,
                observed_at_ms: row.16.map(from_i64).transpose()?,
            })
        })
        .collect()
}

pub(super) fn read_next_unfinished_effect(
    connection: &Connection,
    operation_id: &str,
) -> UseResult<Option<ControlEffectRecord>> {
    let effects = read_effects_from(connection, operation_id)?;
    Ok(effects.into_iter().find(|effect| {
        !matches!(
            effect.status,
            ControlEffectStatus::Applied | ControlEffectStatus::Rejected
        )
    }))
}

pub(super) fn read_effect_by_key(
    connection: &Connection,
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
    Ok(read_effects_from(connection, &operation_id)?
        .into_iter()
        .find(|effect| effect.intent.idempotency_key == idempotency_key))
}

pub(super) fn read_cursors(connection: &Connection) -> UseResult<(u64, u64)> {
    let (generation, capability): (i64, i64) = connection
        .query_row(
            "SELECT current_generation, published_capability_generation
             FROM control_installation WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| schema::sqlite_error("read Control Store cursors", error))?;
    Ok((from_i64(generation)?, from_i64(capability)?))
}

pub(super) fn query_i64_values(
    connection: &Connection,
    query: &str,
    action: &str,
) -> UseResult<Vec<i64>> {
    let mut statement = connection
        .prepare(query)
        .map_err(|error| schema::sqlite_error(action, error))?;
    let values = statement
        .query_map([], |row| row.get::<_, i64>(0))
        .map_err(|error| schema::sqlite_error(action, error))?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| schema::sqlite_error(action, error))?;
    Ok(values)
}
