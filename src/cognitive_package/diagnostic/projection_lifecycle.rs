// Lifecycle/grant diagnostic projection helpers (included into projection).

fn reconstructed_install_generations(
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
) -> UseResult<BTreeMap<String, u64>> {
    let Some(lock) = envelope.package_lock.as_ref() else {
        return Err(diagnostic_state_error());
    };
    let state_revision = envelope.plan.state.state_revision;
    let mut generations = BTreeMap::new();
    for (index, package) in lock
        .install_order()
        .map_err(|_| diagnostic_state_error())?
        .into_iter()
        .enumerate()
    {
        let package_id = package.package_id();
        let changed = envelope.plan.packages.iter().any(|transition| {
            transition.package_id == package_id
                && matches!(
                    transition.change,
                    PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
                )
        });
        if !changed {
            continue;
        }
        let offset = u64::try_from(index).map_err(|_| diagnostic_state_error())?;
        let generation = state_revision
            .checked_add(offset)
            .ok_or_else(diagnostic_state_error)?;
        generations.insert(package_id.to_string(), generation);
    }
    Ok(generations)
}

fn expected_lifecycle_units_for_envelope(
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
    generations: &BTreeMap<String, u64>,
    prior_generations: &BTreeMap<String, u64>,
) -> UseResult<Vec<ExpectedLifecycleUnit>> {
    let mut candidates = Vec::new();
    let mut retirements = Vec::new();
    for transition in &envelope.plan.packages {
        match (envelope.plan.action, transition.change) {
            (PluginOperationAction::Install, PlanPackageChangeKind::Add)
            | (PluginOperationAction::Upgrade, PlanPackageChangeKind::Add) => {
                candidates.push(expected_lifecycle_unit(
                    transition,
                    PluginLifecycleAction::Install,
                    transition.after.as_ref(),
                    generations.get(&transition.package_id).copied(),
                )?);
            }
            (PluginOperationAction::Upgrade, PlanPackageChangeKind::Replace) => {
                candidates.push(expected_lifecycle_unit(
                    transition,
                    PluginLifecycleAction::Upgrade,
                    transition.after.as_ref(),
                    generations.get(&transition.package_id).copied(),
                )?);
                retirements.push(expected_lifecycle_unit(
                    transition,
                    PluginLifecycleAction::Uninstall,
                    transition.before.as_ref(),
                    prior_generations.get(&transition.package_id).copied(),
                )?);
            }
            (PluginOperationAction::Upgrade, PlanPackageChangeKind::Remove)
            | (PluginOperationAction::Uninstall, PlanPackageChangeKind::Remove) => {
                retirements.push(expected_lifecycle_unit(
                    transition,
                    PluginLifecycleAction::Uninstall,
                    transition.before.as_ref(),
                    prior_generations
                        .get(&transition.package_id)
                        .copied()
                        .or_else(|| generations.get(&transition.package_id).copied()),
                )?);
            }
            (
                PluginOperationAction::Install
                | PluginOperationAction::Upgrade
                | PluginOperationAction::Uninstall,
                PlanPackageChangeKind::Retain,
            ) => {}
            _ => return Err(diagnostic_state_error()),
        }
    }
    candidates.extend(retirements);
    if candidates.is_empty() || candidates.len() > MAX_DIAGNOSTIC_LIFECYCLE_UNITS {
        return Err(diagnostic_state_error());
    }
    Ok(candidates)
}

pub(super) fn expected_enablement_lifecycle_units(
    active: &crate::cognitive_package::enablement_store::PendingCognitivePackageEnablement,
) -> UseResult<Vec<ExpectedLifecycleUnit>> {
    let expected_action = if active.request.enabled {
        PluginLifecycleAction::Enable
    } else {
        PluginLifecycleAction::Disable
    };
    if active.intent.action != expected_action
        || active.intent.operation_id != active.envelope.plan.operation_id
        || active.intent.plan_digest != active.envelope.plan_digest
    {
        return Err(diagnostic_state_error());
    }
    Ok(vec![expected_enablement_intent_lifecycle_unit(
        &active.intent,
    )?])
}

pub(super) fn expected_enablement_intent_lifecycle_unit(
    intent: &crate::plugin_lifecycle::PluginLifecycleIntent,
) -> UseResult<ExpectedLifecycleUnit> {
    intent.validate().map_err(|_| diagnostic_state_error())?;
    if !matches!(
        intent.action,
        PluginLifecycleAction::Enable | PluginLifecycleAction::Disable
    ) {
        return Err(diagnostic_state_error());
    }
    Ok(ExpectedLifecycleUnit {
        package_id: intent.package_id.clone(),
        action: intent.action,
        generation: intent.generation,
        package_digest: intent.package_digest.clone(),
        manifest_digest: intent.manifest_digest.clone(),
        total_checkpoints: u32::try_from(intent.checkpoints.len())
            .map_err(|_| diagnostic_state_error())?,
    })
}

pub(super) fn enablement_cutover_key(
    active: &crate::cognitive_package::enablement_store::PendingCognitivePackageEnablement,
) -> UseResult<String> {
    let expected_action = if active.request.enabled {
        PluginLifecycleAction::Enable
    } else {
        PluginLifecycleAction::Disable
    };
    if active.intent.action != expected_action {
        return Err(diagnostic_state_error());
    }
    enablement_intent_cutover_key(&active.intent)
}

pub(super) fn enablement_intent_cutover_key(
    intent: &crate::plugin_lifecycle::PluginLifecycleIntent,
) -> UseResult<String> {
    intent.validate().map_err(|_| diagnostic_state_error())?;
    let kind = match intent.action {
        PluginLifecycleAction::Enable => PluginLifecycleCheckpointKind::CapabilityPublished,
        PluginLifecycleAction::Disable => PluginLifecycleCheckpointKind::CapabilityHidden,
        _ => return Err(diagnostic_state_error()),
    };
    let matching = intent
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.kind == kind)
        .collect::<Vec<_>>();
    let [checkpoint] = matching.as_slice() else {
        return Err(diagnostic_state_error());
    };
    Ok(checkpoint.idempotency_key.clone())
}

fn expected_lifecycle_unit(
    transition: &a3s_use_core::PlannedPackageTransition,
    action: PluginLifecycleAction,
    state: Option<&PlannedPackageState>,
    generation: Option<u64>,
) -> UseResult<ExpectedLifecycleUnit> {
    let state = state.ok_or_else(diagnostic_state_error)?;
    let generation = generation
        .filter(|value| *value > 0)
        .ok_or_else(diagnostic_state_error)?;
    let surface_count =
        u32::try_from(state.release.surfaces.len()).map_err(|_| diagnostic_state_error())?;
    let fixed = match action {
        PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade => 2,
        PluginLifecycleAction::Uninstall => 3,
        PluginLifecycleAction::Enable | PluginLifecycleAction::Disable => {
            return Err(diagnostic_state_error())
        }
    };
    Ok(ExpectedLifecycleUnit {
        package_id: transition.package_id.clone(),
        action,
        generation,
        package_digest: state.release.package_sha256.clone(),
        manifest_digest: state.release.manifest_sha256.clone(),
        total_checkpoints: surface_count
            .checked_add(fixed)
            .ok_or_else(diagnostic_state_error)?,
    })
}

pub(super) async fn observe_lifecycle(
    manager: &CognitivePackageManager,
    operation_id: &str,
    plan_digest: &str,
    phase: PluginOperationDiagnosticPhase,
    expected: &[ExpectedLifecycleUnit],
) -> UseResult<Vec<ObservedLifecycleUnit>> {
    // Control owns lifecycle checkpoints in SQLite. The legacy
    // `operations/plugins` journal is never authority beside Control.
    let _ = (manager, operation_id, plan_digest, phase, expected);
    Ok(Vec::new())
}

fn lifecycle_summary(
    package_id: &str,
    operation: &PluginLifecycleOperationDiagnostic,
) -> UseResult<PluginLifecycleOperationSummary> {
    let publication_kind = match operation.action {
        PluginLifecycleAction::Install | PluginLifecycleAction::Upgrade => {
            PluginLifecycleCheckpointKind::CapabilityPublished
        }
        PluginLifecycleAction::Enable => PluginLifecycleCheckpointKind::CapabilityPublished,
        PluginLifecycleAction::Uninstall | PluginLifecycleAction::Disable => {
            PluginLifecycleCheckpointKind::CapabilityHidden
        }
    };
    let publication_checkpoint = exact_checkpoint(operation, publication_kind)?;
    let publication = match publication_checkpoint.status {
        PluginLifecycleCheckpointDiagnosticStatus::Pending => {
            PluginLifecyclePublicationDiagnosticStatus::Pending
        }
        PluginLifecycleCheckpointDiagnosticStatus::Applied => match publication_kind {
            PluginLifecycleCheckpointKind::CapabilityPublished => {
                PluginLifecyclePublicationDiagnosticStatus::Published
            }
            PluginLifecycleCheckpointKind::CapabilityHidden => {
                PluginLifecyclePublicationDiagnosticStatus::Hidden
            }
            _ => return Err(diagnostic_state_error()),
        },
        PluginLifecycleCheckpointDiagnosticStatus::OptionalFailed
        | PluginLifecycleCheckpointDiagnosticStatus::Failed => {
            PluginLifecyclePublicationDiagnosticStatus::Failed
        }
    };
    let drain = if matches!(
        operation.action,
        PluginLifecycleAction::Uninstall | PluginLifecycleAction::Disable
    ) {
        match exact_checkpoint(operation, PluginLifecycleCheckpointKind::CallsDrained)?.status {
            PluginLifecycleCheckpointDiagnosticStatus::Pending => {
                PluginLifecycleDrainDiagnosticStatus::Pending
            }
            PluginLifecycleCheckpointDiagnosticStatus::Applied => {
                PluginLifecycleDrainDiagnosticStatus::Completed
            }
            PluginLifecycleCheckpointDiagnosticStatus::OptionalFailed
            | PluginLifecycleCheckpointDiagnosticStatus::Failed => {
                PluginLifecycleDrainDiagnosticStatus::Failed
            }
        }
    } else {
        PluginLifecycleDrainDiagnosticStatus::NotRequired
    };
    let current_checkpoint = operation
        .checkpoints
        .iter()
        .find(|checkpoint| {
            matches!(
                checkpoint.status,
                PluginLifecycleCheckpointDiagnosticStatus::Pending
                    | PluginLifecycleCheckpointDiagnosticStatus::Failed
            )
        })
        .cloned();
    Ok(PluginLifecycleOperationSummary {
        package_id: package_id.to_owned(),
        action: operation.action,
        status: operation.status,
        generation: operation.generation,
        intent_digest: operation.intent_digest.clone(),
        completed_checkpoints: operation.completed_checkpoints,
        total_checkpoints: operation.total_checkpoints,
        publication,
        drain,
        current_checkpoint,
        rollback_evidence_digest: operation.rollback_evidence_digest.clone(),
        completed_at_ms: operation.completed_at_ms,
    })
}

fn exact_checkpoint(
    operation: &PluginLifecycleOperationDiagnostic,
    kind: PluginLifecycleCheckpointKind,
) -> UseResult<&PluginLifecycleCheckpointDiagnostic> {
    let checkpoints = operation
        .checkpoints
        .iter()
        .filter(|checkpoint| checkpoint.kind == kind)
        .collect::<Vec<_>>();
    let [checkpoint] = checkpoints.as_slice() else {
        return Err(diagnostic_state_error());
    };
    Ok(checkpoint)
}

pub(super) fn project_providers(
    plan: &a3s_use_core::PluginOperationPlan,
    lifecycle: &[ObservedLifecycleUnit],
) -> UseResult<Vec<PluginProviderOperationDiagnostic>> {
    plan.providers
        .iter()
        .map(|provider| {
            let matching_operation = lifecycle.iter().find(|unit| {
                unit.summary.package_id == provider.surface.package_id
                    && matches!(
                        unit.summary.action,
                        PluginLifecycleAction::Install
                            | PluginLifecycleAction::Upgrade
                            | PluginLifecycleAction::Enable
                    )
            });
            let readiness = if let Some(operation) = matching_operation {
                let checkpoints = operation
                    .raw
                    .checkpoints
                    .iter()
                    .filter(|checkpoint| {
                        checkpoint.kind == PluginLifecycleCheckpointKind::SurfacePrepared
                            && checkpoint.surface.as_ref() == Some(&provider.surface.surface)
                    })
                    .collect::<Vec<_>>();
                let [checkpoint] = checkpoints.as_slice() else {
                    return Err(diagnostic_state_error());
                };
                match checkpoint.status {
                    PluginLifecycleCheckpointDiagnosticStatus::Pending => {
                        PluginProviderDiagnosticReadiness::Preparing
                    }
                    PluginLifecycleCheckpointDiagnosticStatus::Applied => {
                        PluginProviderDiagnosticReadiness::Ready
                    }
                    PluginLifecycleCheckpointDiagnosticStatus::OptionalFailed => {
                        PluginProviderDiagnosticReadiness::OptionalFailed
                    }
                    PluginLifecycleCheckpointDiagnosticStatus::Failed => {
                        PluginProviderDiagnosticReadiness::Failed
                    }
                }
            } else {
                PluginProviderDiagnosticReadiness::Selected
            };
            Ok(PluginProviderOperationDiagnostic {
                surface: provider.surface.clone(),
                provider_id: provider.provider_id.clone(),
                provider_build_id: provider.provider_build_id.clone(),
                capability_digest: provider.capability_digest.clone(),
                semantics_profile_digest: provider.semantics_profile_digest.clone(),
                enforcement: provider.enforcement,
                readiness,
            })
        })
        .collect()
}

pub(super) async fn observe_grant(
    manager: &CognitivePackageManager,
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
    authorization: &crate::cognitive_package::grant::PackageGraphAuthorization,
    phase: PluginOperationDiagnosticPhase,
) -> UseResult<PluginGrantOperationDiagnostic> {
    let required =
        envelope.plan.workspace_impacts.iter().any(|impact| {
            impact.grant_before_digest.is_some() || impact.grant_after_digest.is_some()
        });
    match phase {
        PluginOperationDiagnosticPhase::Planned | PluginOperationDiagnosticPhase::Cancelled => {
            // Do not open the legacy `grants/` leaf for pre-admission projection.
            Ok(empty_grant_diagnostic(
                required,
                if phase == PluginOperationDiagnosticPhase::Cancelled {
                    PluginGrantDiagnosticStatus::Cancelled
                } else if required {
                    PluginGrantDiagnosticStatus::AwaitingAdmission
                } else {
                    PluginGrantDiagnosticStatus::NotRequired
                },
            ))
        }
        PluginOperationDiagnosticPhase::Admitted => {
            // Control-native: project from reviewed authorization evidence only.
            // Never open the legacy `grants/` leaf.
            let resolved = authorization.resolved_grants.as_ref();
            if required != resolved.is_some() {
                return Err(diagnostic_state_error());
            }
            let Some(resolved) = resolved else {
                return Ok(empty_grant_diagnostic(
                    false,
                    PluginGrantDiagnosticStatus::NotRequired,
                ));
            };
            Ok(PluginGrantOperationDiagnostic {
                required: true,
                status: PluginGrantDiagnosticStatus::Authorized,
                candidate_count: bounded_count(resolved.grants.len(), "Grant candidate")?,
                retirement_count: bounded_count(resolved.revocations.len(), "Grant retirement")?,
                change_set_digest: Some(resolved.change_set_digest.clone()),
                intent_digest: None,
                state_revision_before: Some(resolved.state_revision_before),
                state_revision_after: Some(resolved.revision),
                capability_generation_before: Some(resolved.capability_generation_before),
                capability_generation_after: Some(resolved.capability_generation_after),
                transitioned_at_ms: Some(resolved.transitioned_at_ms),
                cutover_snapshot_digest: None,
                cutover_committed_at_ms: None,
                rollback_evidence_digest: None,
                rolled_back_at_ms: None,
            })
        }
    }
}

fn validate_grant_journal(
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
    authorization: &crate::cognitive_package::grant::PackageGraphAuthorization,
    journal: &WorkspaceGrantOperationJournal,
) -> UseResult<()> {
    if journal.intent.operation_id != envelope.plan.operation_id
        || journal.intent.plan_digest != envelope.plan_digest
        || journal.intent.scope_id != envelope.plan.scope.id
        || journal.intent.state_revision_before != envelope.plan.state.state_revision
        || authorization
            .resolved_grants
            .as_ref()
            .is_some_and(|resolved| {
                resolved.change_set_digest != journal.intent.change_set_digest
                    || resolved.revision != journal.intent.revision
                    || resolved.capability_generation_before
                        != journal.intent.capability_generation_before
                    || resolved.capability_generation_after
                        != journal.intent.capability_generation_after
            })
    {
        return Err(diagnostic_state_error());
    }
    Ok(())
}

pub(super) fn empty_grant_diagnostic(
    required: bool,
    status: PluginGrantDiagnosticStatus,
) -> PluginGrantOperationDiagnostic {
    PluginGrantOperationDiagnostic {
        required,
        status,
        candidate_count: 0,
        retirement_count: 0,
        change_set_digest: None,
        intent_digest: None,
        state_revision_before: None,
        state_revision_after: None,
        capability_generation_before: None,
        capability_generation_after: None,
        transitioned_at_ms: None,
        cutover_snapshot_digest: None,
        cutover_committed_at_ms: None,
        rollback_evidence_digest: None,
        rolled_back_at_ms: None,
    }
}

fn grant_status(phase: WorkspaceGrantLifecyclePhase) -> PluginGrantDiagnosticStatus {
    match phase {
        WorkspaceGrantLifecyclePhase::IntentRecorded => PluginGrantDiagnosticStatus::IntentRecorded,
        WorkspaceGrantLifecyclePhase::Preparing => PluginGrantDiagnosticStatus::Preparing,
        WorkspaceGrantLifecyclePhase::Prepared => PluginGrantDiagnosticStatus::Prepared,
        WorkspaceGrantLifecyclePhase::CutoverCommitted => {
            PluginGrantDiagnosticStatus::CutoverCommitted
        }
        WorkspaceGrantLifecyclePhase::Retiring => PluginGrantDiagnosticStatus::Retiring,
        WorkspaceGrantLifecyclePhase::Completed => PluginGrantDiagnosticStatus::Completed,
        WorkspaceGrantLifecyclePhase::RollingBack => PluginGrantDiagnosticStatus::RollingBack,
        WorkspaceGrantLifecyclePhase::RolledBack => PluginGrantDiagnosticStatus::RolledBack,
    }
}

pub(super) fn project_registry_cutover(
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
    phase: PluginOperationDiagnosticPhase,
    cutover_key: &str,
    records: &[ExtensionRegistryCutoverRecord],
    current_generation: u64,
    lifecycle: &[ObservedLifecycleUnit],
    grant: &PluginGrantOperationDiagnostic,
) -> UseResult<PluginRegistryCutoverDiagnostic> {
    let expected_generation_before = envelope.plan.state.capability_generation;
    let expected_generation_after = expected_generation_before
        .checked_add(1)
        .ok_or_else(diagnostic_state_error)?;
    let matching = records
        .iter()
        .filter(|record| record.idempotency_key == cutover_key)
        .collect::<Vec<_>>();
    let record = match matching.as_slice() {
        [] => None,
        [record] => Some(*record),
        _ => return Err(diagnostic_state_error()),
    };
    let publication_observed = lifecycle.iter().any(|unit| {
        matches!(
            unit.summary.publication,
            PluginLifecyclePublicationDiagnosticStatus::Published
                | PluginLifecyclePublicationDiagnosticStatus::Hidden
        )
    });
    let grant_cutover = grant
        .capability_generation_before
        .zip(grant.capability_generation_after)
        .zip(grant.cutover_snapshot_digest.as_deref());
    if phase != PluginOperationDiagnosticPhase::Admitted
        && (record.is_some() || publication_observed || grant_cutover.is_some())
    {
        return Err(diagnostic_state_error());
    }

    let (status, recorded_generation_after, recorded_snapshot_digest) = if let Some(record) = record
    {
        if record.registry_generation_before != expected_generation_before
            || record.registry_generation_after != expected_generation_after
            || grant_cutover.is_some_and(|((before, after), digest)| {
                before != record.registry_generation_before
                    || after != record.registry_generation_after
                    || digest != record.registry_snapshot_digest
            })
        {
            return Err(diagnostic_state_error());
        }
        (
            PluginRegistryCutoverDiagnosticStatus::Recorded,
            Some(record.registry_generation_after),
            Some(record.registry_snapshot_digest.clone()),
        )
    } else if let Some(((before, after), digest)) = grant_cutover {
        if before != expected_generation_before
            || after != expected_generation_after
            || current_generation < after
        {
            return Err(diagnostic_state_error());
        }
        (
            if current_generation == after {
                PluginRegistryCutoverDiagnosticStatus::Acknowledged
            } else {
                PluginRegistryCutoverDiagnosticStatus::Superseded
            },
            Some(after),
            Some(digest.to_owned()),
        )
    } else if publication_observed {
        if current_generation < expected_generation_after {
            return Err(diagnostic_state_error());
        }
        (
            if current_generation == expected_generation_after {
                PluginRegistryCutoverDiagnosticStatus::Acknowledged
            } else {
                PluginRegistryCutoverDiagnosticStatus::Superseded
            },
            Some(expected_generation_after),
            None,
        )
    } else if current_generation == expected_generation_before {
        (
            PluginRegistryCutoverDiagnosticStatus::NotObserved,
            None,
            None,
        )
    } else if current_generation > expected_generation_before {
        (
            PluginRegistryCutoverDiagnosticStatus::GenerationDrift,
            None,
            None,
        )
    } else {
        return Err(diagnostic_state_error());
    };
    Ok(PluginRegistryCutoverDiagnostic {
        status,
        expected_generation_before,
        expected_generation_after,
        recorded_generation_after,
        recorded_snapshot_digest,
    })
}

pub(super) fn confirmation_status(
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
    authorization: &crate::cognitive_package::grant::PackageGraphAuthorization,
    phase: PluginOperationDiagnosticPhase,
) -> PluginOperationConfirmationDiagnosticStatus {
    if phase == PluginOperationDiagnosticPhase::Cancelled {
        return PluginOperationConfirmationDiagnosticStatus::Cancelled;
    }
    if !envelope.plan.authority.confirmation_required {
        return PluginOperationConfirmationDiagnosticStatus::NotRequired;
    }
    if authorization.operation_confirmation.is_some() {
        PluginOperationConfirmationDiagnosticStatus::Confirmed
    } else {
        PluginOperationConfirmationDiagnosticStatus::AwaitingConfirmation
    }
}

pub(super) fn diagnostic_phase(
    phase: PackageGraphOperationPhase,
) -> PluginOperationDiagnosticPhase {
    match phase {
        PackageGraphOperationPhase::Planned => PluginOperationDiagnosticPhase::Planned,
        PackageGraphOperationPhase::Admitted => PluginOperationDiagnosticPhase::Admitted,
        PackageGraphOperationPhase::Cancelled => PluginOperationDiagnosticPhase::Cancelled,
    }
}
