use super::*;

pub(super) fn read_projection_history(
    connection: &Connection,
    through_generation: u64,
) -> UseResult<ControlProjectionHistory> {
    let last_lifecycle_generation = from_i64(
        connection
            .query_row(
                "SELECT COALESCE(MAX(lifecycle_generation), 0)
                 FROM selected_package WHERE generation <= ?1",
                [to_i64(through_generation)?],
                |row| row.get(0),
            )
            .map_err(|error| schema::sqlite_error("read Control Store lifecycle history", error))?,
    )?;
    let mut statement = connection
        .prepare(
            "SELECT package_id, MAX(state_generation)
             FROM selected_package
             WHERE generation <= ?1
             GROUP BY package_id
             ORDER BY package_id
             LIMIT ?2",
        )
        .map_err(|error| schema::sqlite_error("prepare package state history", error))?;
    let limit = i64::try_from(super::MAX_CONTROL_HISTORY_PACKAGES)
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| corruption_error("The package history bound is invalid."))?;
    let rows = statement
        .query_map(params![to_i64(through_generation)?, limit], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| schema::sqlite_error("query package state history", error))?;
    let mut package_state_generations = std::collections::BTreeMap::new();
    for row in rows {
        let (package_id, generation) =
            row.map_err(|error| schema::sqlite_error("read package state history", error))?;
        if package_state_generations.len() >= super::MAX_CONTROL_HISTORY_PACKAGES
            || package_state_generations
                .insert(package_id, from_i64(generation)?)
                .is_some()
        {
            return Err(corruption_error(
                "The Control Store package state history is duplicated or exceeds its bound.",
            ));
        }
    }
    ControlProjectionHistory::new(last_lifecycle_generation, package_state_generations)
        .map_err(|_| corruption_error("The Control Store package state history is invalid."))
}

pub(super) fn insert_generation(
    transaction: &Transaction<'_>,
    transition: &ControlTransition,
) -> UseResult<()> {
    let generation = transition.snapshot.generation;
    let snapshot_json = transition.snapshot.canonical_bytes()?;
    let snapshot_digest = transition.snapshot.descriptor_digest()?;
    transaction
        .execute(
            "INSERT INTO control_generation(
                generation, operation_id, snapshot_json, snapshot_digest, committed_at_ms
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                to_i64(generation)?,
                transition.operation_id,
                snapshot_json,
                snapshot_digest,
                to_i64(transition.committed_at_ms)?,
            ],
        )
        .map_err(|error| mutation_error("insert Control Store generation", error))?;
    for (package, lifecycle) in transition
        .snapshot
        .packages
        .iter()
        .zip(&transition.package_lifecycles)
    {
        let package_json = canonical_json(&package.package)?;
        let package_digest = sha256_digest(&package_json);
        transaction
            .execute(
                "INSERT INTO selected_package(
                    generation, package_id, lifecycle_generation, state_generation,
                    enabled, package_json, package_digest
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    to_i64(generation)?,
                    package.package_id(),
                    to_i64(lifecycle.lifecycle_generation)?,
                    to_i64(package.state_generation)?,
                    package.enabled,
                    package_json,
                    package_digest,
                ],
            )
            .map_err(|error| mutation_error("insert selected Control Store package", error))?;
    }
    // Dependency edges have immediate foreign keys to both selected package
    // rows. Persist the complete node set first so correctness is independent
    // of lexical package IDs or graph topology while remaining in one
    // transaction.
    for package in &transition.snapshot.packages {
        for dependency in &package.package.dependencies {
            transaction
                .execute(
                    "INSERT INTO package_dependency(
                        generation, package_id, dependency_package_id,
                        version_requirement, selected_version
                     ) VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![
                        to_i64(generation)?,
                        package.package_id(),
                        dependency.package_id,
                        dependency.version_requirement,
                        dependency.version,
                    ],
                )
                .map_err(|error| mutation_error("insert Control Store dependency", error))?;
        }
        for surface in &package.selected_surfaces {
            transaction
                .execute(
                    "INSERT INTO selected_surface(
                        generation, package_id, surface_kind, surface_id
                     ) VALUES (?1, ?2, ?3, ?4)",
                    params![
                        to_i64(generation)?,
                        package.package_id(),
                        surface_kind_name(surface.kind),
                        surface.id,
                    ],
                )
                .map_err(|error| mutation_error("insert selected Control Store surface", error))?;
        }
    }
    for root in &transition.snapshot.roots {
        transaction
            .execute(
                "INSERT INTO installation_root(generation, package_id, installed_at_ms)
                 VALUES (?1, ?2, ?3)",
                params![
                    to_i64(generation)?,
                    root.package_id,
                    to_i64(root.installed_at_ms)?,
                ],
            )
            .map_err(|error| mutation_error("insert Control Store root", error))?;
    }
    for grant in &transition.grants {
        let grant_json = grant.grant.canonical_bytes()?;
        transaction
            .execute(
                "INSERT INTO control_grant(
                    generation, package_id, receipt_revision, grant_json, grant_digest
                 ) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    to_i64(generation)?,
                    grant.package_id(),
                    to_i64(grant.receipt_revision)?,
                    grant_json,
                    grant.grant_digest,
                ],
            )
            .map_err(|error| mutation_error("insert Control Store Grant", error))?;
    }
    for selection in &transition.provider_selections {
        transaction
            .execute(
                "INSERT INTO provider_selection(
                    generation, package_id, surface_kind, surface_id,
                    provider_id, provider_build_id, capability_digest,
                    semantics_profile_digest, enforcement_profile, selection_digest
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    to_i64(generation)?,
                    selection.package_id(),
                    surface_kind_name(selection.surface().kind),
                    selection.surface().id,
                    selection.evidence.provider_id,
                    selection.evidence.provider_build_id,
                    selection.evidence.capability_digest,
                    selection.evidence.semantics_profile_digest,
                    enforcement_profile_name(selection.evidence.enforcement),
                    selection.selection_digest,
                ],
            )
            .map_err(|error| mutation_error("insert Control Store provider selection", error))?;
    }
    transaction
        .execute(
            "INSERT INTO capability_generation(
                capability_generation, installation_generation,
                descriptor_digest, publication_state
             ) VALUES (?1, ?2, ?3, 'candidate')",
            params![
                to_i64(transition.capability.generation)?,
                to_i64(generation)?,
                transition.capability.descriptor_digest,
            ],
        )
        .map_err(|error| mutation_error("insert Control Store capability generation", error))?;
    for effect in &transition.effects {
        let (package_id, package_lifecycle_generation) = effect
            .subject
            .package_identity()
            .map_or((None, None), |(package_id, generation)| {
                (Some(package_id), Some(generation))
            });
        let (surface_kind, surface_id) = effect.subject.surface().map_or((None, None), |surface| {
            (
                Some(surface_kind_name(surface.kind)),
                Some(surface.id.as_str()),
            )
        });
        let payload_json = effect.canonical_bytes()?;
        let payload_digest = effect.descriptor_digest()?;
        transaction
            .execute(
                "INSERT INTO lifecycle_checkpoint(
                    operation_id, sequence, installation_generation, subject_kind,
                    package_id, package_lifecycle_generation, surface_kind, surface_id,
                    checkpoint_kind, required
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    transition.operation_id,
                    i64::from(effect.sequence),
                    to_i64(effect.installation_generation)?,
                    effect.subject.kind_name(),
                    package_id,
                    package_lifecycle_generation.map(to_i64).transpose()?,
                    surface_kind,
                    surface_id,
                    effect.kind.as_str(),
                    effect.required,
                ],
            )
            .map_err(|error| mutation_error("insert Control Store checkpoint", error))?;
        transaction
            .execute(
                "INSERT INTO effect_outbox(
                    idempotency_key, operation_id, sequence, owner_kind,
                    payload_json, payload_digest, status, attempt
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'pending', 0)",
                params![
                    effect.idempotency_key,
                    transition.operation_id,
                    i64::from(effect.sequence),
                    effect.owner.kind_name(),
                    payload_json,
                    payload_digest,
                ],
            )
            .map_err(|error| mutation_error("insert Control Store outbox effect", error))?;
    }
    Ok(())
}
