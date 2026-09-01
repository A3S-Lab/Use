use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{InstallationId, PluginSurfaceRef, UseResult};

use super::*;
use crate::control_store::model::{
    ControlCapabilityEffectAuthority, ControlCapabilitySurfaceAuthority,
    ControlCapabilitySurfaceState, ControlEffectAuthority, ControlEffectOwner,
    ControlPackageEffectAuthority, ControlRuntimeEffectAuthority,
};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SurfaceIncarnation {
    package_id: String,
    lifecycle_generation: u64,
    surface: PluginSurfaceRef,
}

pub(super) fn derive_claim_authority(
    connection: &Connection,
    installation: &InstallationId,
    operation: &ControlOperationRecord,
    effect: &ControlEffectRecord,
) -> UseResult<ControlEffectAuthority> {
    let generation = read_generation_from(
        connection,
        installation,
        effect.intent.installation_generation,
    )?
    .ok_or_else(|| {
        corruption_error("A claimed Control effect references a missing committed generation.")
    })?;

    match &effect.intent.owner {
        ControlEffectOwner::CapabilityIndex => {
            if generation.operation_id != operation.reviewed.operation_id()
                || generation.snapshot.generation != operation.reviewed.target_generation()?
                || generation.capability_status != ControlCapabilityStatus::Candidate
            {
                return Err(corruption_error(
                    "A Capability Index claim does not reference its candidate Control generation.",
                ));
            }
            validate_generation_grant_coverage(&generation)?;
            let history = super::authority_from(connection, installation)?;
            let materializations = materializations_for(
                &generation,
                operation,
                effect,
                &history.operations,
                &history.effects,
            )?;
            Ok(ControlEffectAuthority::CapabilityIndex(
                ControlCapabilityEffectAuthority {
                    generation,
                    materializations,
                },
            ))
        }
        ControlEffectOwner::InvocationLeases => Ok(ControlEffectAuthority::InvocationLeases(
            package_authority(&generation, &effect.intent)?,
        )),
        ControlEffectOwner::RuntimeProvider { .. } => {
            let package = package_authority(&generation, &effect.intent)?;
            let surface = effect.intent.subject.surface().ok_or_else(|| {
                corruption_error("A Runtime effect has no committed surface identity.")
            })?;
            let selection = generation
                .provider_selections
                .iter()
                .find(|selection| {
                    selection.package_id() == package.package.package_id()
                        && selection.surface() == surface
                })
                .cloned()
                .ok_or_else(|| {
                    corruption_error("A Runtime effect has no exact committed provider selection.")
                })?;
            if !effect.intent.owner.matches_generation(
                &effect.intent.subject,
                effect.intent.kind,
                std::slice::from_ref(&selection),
            ) {
                return Err(corruption_error(
                    "A Runtime effect owner differs from its committed provider selection.",
                ));
            }
            Ok(ControlEffectAuthority::RuntimeProvider(
                ControlRuntimeEffectAuthority {
                    package,
                    provider_selection: selection,
                },
            ))
        }
        ControlEffectOwner::FlowHost => Ok(ControlEffectAuthority::FlowHost(package_authority(
            &generation,
            &effect.intent,
        )?)),
        ControlEffectOwner::KnowledgeHost => Ok(ControlEffectAuthority::KnowledgeHost(
            package_authority(&generation, &effect.intent)?,
        )),
        ControlEffectOwner::SkillHost => Ok(ControlEffectAuthority::SkillHost(package_authority(
            &generation,
            &effect.intent,
        )?)),
        ControlEffectOwner::UiHost => Ok(ControlEffectAuthority::UiHost(package_authority(
            &generation,
            &effect.intent,
        )?)),
    }
}

fn package_authority(
    generation: &ControlGeneration,
    intent: &ControlEffectIntent,
) -> UseResult<ControlPackageEffectAuthority> {
    let (package_id, lifecycle_generation) = intent
        .subject
        .package_identity()
        .ok_or_else(|| corruption_error("A package owner received an installation effect."))?;
    let package = generation
        .snapshot
        .package_selection(package_id)
        .cloned()
        .ok_or_else(|| {
            corruption_error("A claimed Control effect package is absent from its generation.")
        })?;
    let committed_lifecycle = generation
        .package_lifecycles
        .binary_search_by(|candidate| candidate.package_id.as_str().cmp(package_id))
        .ok()
        .and_then(|index| generation.package_lifecycles.get(index))
        .ok_or_else(|| {
            corruption_error("A claimed Control effect package has no lifecycle incarnation.")
        })?;
    if committed_lifecycle.lifecycle_generation != lifecycle_generation
        || (intent.kind == ControlEffectKind::SurfacePrepare && !package.enabled)
    {
        return Err(corruption_error(
            "A claimed Control effect differs from its committed package incarnation.",
        ));
    }
    let grant = generation
        .grants
        .binary_search_by(|candidate| candidate.package_id().cmp(package_id))
        .ok()
        .and_then(|index| generation.grants.get(index))
        .cloned();
    if grant_required(&package)? != grant.is_some() {
        return Err(corruption_error(
            "A claimed Control effect package has incomplete committed Grant authority.",
        ));
    }
    let authority = ControlPackageEffectAuthority {
        generation_operation_id: generation.operation_id.clone(),
        installation_generation: generation.snapshot.generation,
        snapshot_digest: generation.snapshot_digest.clone(),
        committed_at_ms: generation.committed_at_ms,
        host: generation.snapshot.host.clone(),
        package,
        lifecycle_generation,
        grant,
    };
    validate_package_authority(&authority, intent)?;
    Ok(authority)
}

fn validate_generation_grant_coverage(generation: &ControlGeneration) -> UseResult<()> {
    let expected = generation
        .snapshot
        .packages
        .iter()
        .map(|package| Ok((package.package_id(), grant_required(package)?)))
        .filter_map(|result| match result {
            Ok((package_id, true)) => Some(Ok(package_id)),
            Ok((_, false)) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<UseResult<BTreeSet<_>>>()?;
    let actual = generation
        .grants
        .iter()
        .map(ControlGrantSelection::package_id)
        .collect::<BTreeSet<_>>();
    if expected != actual || actual.len() != generation.grants.len() {
        return Err(corruption_error(
            "A Capability Index claim has incomplete committed Grant authority.",
        ));
    }
    Ok(())
}

fn grant_required(package: &a3s_use_core::InstallationPackageSelection) -> UseResult<bool> {
    let selected = package
        .package
        .catalog
        .selected_state(&package.selected_surfaces)
        .map_err(|_| {
            corruption_error("A committed package cannot reconstruct its selected permissions.")
        })?;
    Ok(package.enabled && !selected.permissions.surfaces.is_empty())
}

fn validate_package_authority(
    authority: &ControlPackageEffectAuthority,
    intent: &ControlEffectIntent,
) -> UseResult<()> {
    let (package_id, lifecycle_generation, package_digest, manifest_digest, surface) =
        match &intent.subject {
            ControlEffectSubject::Package {
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                ..
            } => (
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                None,
            ),
            ControlEffectSubject::Surface {
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                surface,
                ..
            } => (
                package_id,
                lifecycle_generation,
                package_digest,
                manifest_digest,
                Some(surface),
            ),
            ControlEffectSubject::Installation { .. } => {
                return Err(corruption_error(
                    "A package authority cannot bind an installation effect.",
                ))
            }
        };
    let catalog_package = &authority.package.package.catalog.record.package;
    let grant_matches = authority.grant.as_ref().is_none_or(|selection| {
        selection.package_id() == package_id
            && selection.grant.package_digest == *package_digest
            && selection.grant.descriptor_digest().is_ok_and(|digest| {
                digest == selection.grant_digest
                    && selection
                        .grant
                        .validate_against(
                            &authority.package.package.catalog.record.permission_ceiling,
                        )
                        .is_ok()
            })
    });
    if !valid_machine_id(&authority.generation_operation_id)
        || authority.installation_generation != intent.installation_generation
        || !valid_sha256(&authority.snapshot_digest)
        || authority.committed_at_ms == 0
        || authority.host.validate().is_err()
        || authority.package.validate().is_err()
        || authority.package.package_id() != package_id
        || authority.lifecycle_generation != *lifecycle_generation
        || catalog_package.sha256.as_deref() != Some(package_digest)
        || catalog_package.manifest_sha256.as_deref() != Some(manifest_digest)
        || surface.is_some_and(|surface| !authority.package.selected_surfaces.contains(surface))
        || !grant_matches
    {
        return Err(corruption_error(
            "A claimed Control effect package authority differs from its committed intent.",
        ));
    }
    Ok(())
}

fn materializations_for(
    generation: &ControlGeneration,
    operation: &ControlOperationRecord,
    cutover: &ControlEffectRecord,
    operations: &[ControlOperationRecord],
    effects: &[ControlEffectRecord],
) -> UseResult<Vec<ControlCapabilitySurfaceAuthority>> {
    let expected = expected_surfaces(generation)?;
    if expected.is_empty() {
        return Ok(Vec::new());
    }
    let operation_targets = operations
        .iter()
        .filter(|candidate| candidate.committed_at_ms.is_some())
        .map(|candidate| {
            Ok((
                candidate.reviewed.operation_id(),
                candidate.reviewed.target_generation()?,
            ))
        })
        .collect::<UseResult<BTreeMap<_, _>>>()?;
    let current_target = operation.reviewed.target_generation()?;
    let mut latest = BTreeMap::<SurfaceIncarnation, (u64, u32, &ControlEffectRecord)>::new();
    for candidate in effects {
        let ControlEffectSubject::Surface {
            package_id,
            lifecycle_generation,
            surface,
            ..
        } = &candidate.intent.subject
        else {
            continue;
        };
        let key = SurfaceIncarnation {
            package_id: package_id.clone(),
            lifecycle_generation: *lifecycle_generation,
            surface: surface.clone(),
        };
        if !expected.contains_key(&key) {
            continue;
        }
        let target = operation_targets
            .get(candidate.operation_id.as_str())
            .copied()
            .ok_or_else(|| {
                corruption_error("A surface effect has no committed Control operation.")
            })?;
        if target > current_target
            || (candidate.operation_id == cutover.operation_id
                && candidate.intent.sequence >= cutover.intent.sequence)
        {
            continue;
        }
        let replace = latest
            .get(&key)
            .is_none_or(|(prior_target, prior_sequence, _)| {
                (target, candidate.intent.sequence) > (*prior_target, *prior_sequence)
            });
        if replace {
            latest.insert(key, (target, candidate.intent.sequence, candidate));
        }
    }

    expected
        .into_keys()
        .map(|key| {
            let record = latest
                .remove(&key)
                .ok_or_else(|| {
                    corruption_error(
                        "A target capability surface has no terminal committed preparation.",
                    )
                })?
                .2;
            if record.intent.kind != ControlEffectKind::SurfacePrepare {
                return Err(corruption_error(
                    "A target capability surface is not in a prepared terminal state.",
                ));
            }
            let observed_at_ms = record.observed_at_ms.ok_or_else(|| {
                corruption_error("A terminal capability surface omitted its observation time.")
            })?;
            let state = match record.status {
                ControlEffectStatus::Applied => {
                    let application = record.application.clone().ok_or_else(|| {
                        corruption_error(
                            "An applied capability surface omitted its typed evidence.",
                        )
                    })?;
                    application.validate_for(&record.intent).map_err(|_| {
                        corruption_error(
                            "An applied capability surface has invalid typed evidence.",
                        )
                    })?;
                    ControlCapabilitySurfaceState::Prepared {
                        application,
                        observed_at_ms,
                    }
                }
                ControlEffectStatus::Rejected if !record.intent.required => {
                    ControlCapabilitySurfaceState::Degraded {
                        evidence_digest: record.evidence_digest.clone().ok_or_else(|| {
                            corruption_error(
                                "A degraded capability surface omitted diagnostic evidence.",
                            )
                        })?,
                        error_code: record.error_code.clone().ok_or_else(|| {
                            corruption_error(
                                "A degraded capability surface omitted its error code.",
                            )
                        })?,
                        observed_at_ms,
                    }
                }
                ControlEffectStatus::Rejected => {
                    return Err(corruption_error(
                        "A required rejected surface cannot enter Capability Index authority.",
                    ))
                }
                ControlEffectStatus::Pending
                | ControlEffectStatus::Claimed
                | ControlEffectStatus::Unknown => {
                    return Err(corruption_error(
                        "A nonterminal surface entered Capability Index authority.",
                    ))
                }
            };
            validate_materialization(generation, &key, record, &state)?;
            Ok(ControlCapabilitySurfaceAuthority {
                intent: record.intent.clone(),
                state,
            })
        })
        .collect()
}

fn expected_surfaces(
    generation: &ControlGeneration,
) -> UseResult<BTreeMap<SurfaceIncarnation, ()>> {
    let lifecycles = generation
        .package_lifecycles
        .iter()
        .map(|lifecycle| {
            (
                lifecycle.package_id.as_str(),
                lifecycle.lifecycle_generation,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut expected = BTreeMap::new();
    for package in generation
        .snapshot
        .packages
        .iter()
        .filter(|package| package.enabled)
    {
        let lifecycle_generation =
            lifecycles
                .get(package.package_id())
                .copied()
                .ok_or_else(|| {
                    corruption_error("A target capability package has no lifecycle incarnation.")
                })?;
        for surface in &package.selected_surfaces {
            if expected
                .insert(
                    SurfaceIncarnation {
                        package_id: package.package_id().to_string(),
                        lifecycle_generation,
                        surface: surface.clone(),
                    },
                    (),
                )
                .is_some()
            {
                return Err(corruption_error(
                    "A target capability surface appears more than once.",
                ));
            }
        }
    }
    Ok(expected)
}

fn validate_materialization(
    generation: &ControlGeneration,
    key: &SurfaceIncarnation,
    record: &ControlEffectRecord,
    state: &ControlCapabilitySurfaceState,
) -> UseResult<()> {
    let ControlEffectSubject::Surface {
        package_digest,
        manifest_digest,
        ..
    } = &record.intent.subject
    else {
        return Err(corruption_error(
            "A capability materialization has a non-surface intent.",
        ));
    };
    let package = generation
        .snapshot
        .package_selection(&key.package_id)
        .ok_or_else(|| corruption_error("A materialized package is absent from the target."))?;
    let catalog_package = &package.package.catalog.record.package;
    let state_valid = match state {
        ControlCapabilitySurfaceState::Prepared { observed_at_ms, .. } => *observed_at_ms > 0,
        ControlCapabilitySurfaceState::Degraded {
            evidence_digest,
            error_code,
            observed_at_ms,
        } => {
            !record.intent.required
                && valid_sha256(evidence_digest)
                && valid_error_code(error_code)
                && *observed_at_ms > 0
        }
    };
    if record.intent.installation != generation.snapshot.installation
        || record.intent.kind != ControlEffectKind::SurfacePrepare
        || record.intent.subject.package_identity()
            != Some((key.package_id.as_str(), key.lifecycle_generation))
        || record.intent.subject.surface() != Some(&key.surface)
        || catalog_package.sha256.as_deref() != Some(package_digest)
        || catalog_package.manifest_sha256.as_deref() != Some(manifest_digest)
        || !record.intent.owner.matches_generation(
            &record.intent.subject,
            record.intent.kind,
            &generation.provider_selections,
        )
        || !state_valid
    {
        return Err(corruption_error(
            "A capability materialization differs from its target Control generation.",
        ));
    }
    Ok(())
}
