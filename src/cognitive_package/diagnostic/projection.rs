use super::*;

pub(super) fn project_sources(
    plan: &a3s_use_core::PluginOperationPlan,
) -> UseResult<Vec<PluginOperationSourceDiagnostic>> {
    plan.packages
        .iter()
        .filter_map(|package| {
            package.source.as_ref().map(|source| match source {
                PluginPlanSource::Registry {
                    provenance,
                    archive,
                } => PluginOperationSourceDiagnostic::Registry {
                    package_id: package.package_id.clone(),
                    registry_name: provenance.registry_name.clone(),
                    root_version: provenance.root_version,
                    timestamp_version: provenance.timestamp_version,
                    snapshot_version: provenance.snapshot_version,
                    targets_version: provenance.targets_version,
                    catalog_record_digest: provenance.catalog_record_digest.clone(),
                    archive_digest: archive.sha256.clone(),
                },
                PluginPlanSource::ReleaseBundle {
                    bundle_digest,
                    package_digest,
                } => PluginOperationSourceDiagnostic::ReleaseBundle {
                    package_id: package.package_id.clone(),
                    bundle_digest: bundle_digest.clone(),
                    package_digest: package_digest.clone(),
                },
                PluginPlanSource::LocalReviewed {
                    source_digest,
                    package_digest,
                    unsigned,
                } => PluginOperationSourceDiagnostic::LocalReviewed {
                    package_id: package.package_id.clone(),
                    source_digest: source_digest.clone(),
                    package_digest: package_digest.clone(),
                    unsigned: *unsigned,
                },
            })
        })
        .map(Ok)
        .collect()
}

pub(super) fn project_installed_source(
    package_id: &str,
    catalog: &a3s_use_core::VerifiedPluginCatalogRecord,
) -> UseResult<Vec<PluginOperationSourceDiagnostic>> {
    catalog.validate().map_err(|_| diagnostic_state_error())?;
    if catalog.record.package_id != package_id {
        return Err(diagnostic_state_error());
    }
    Ok(vec![PluginOperationSourceDiagnostic::Registry {
        package_id: package_id.to_owned(),
        registry_name: catalog.provenance.registry_name.clone(),
        root_version: catalog.provenance.root_version,
        timestamp_version: catalog.provenance.timestamp_version,
        snapshot_version: catalog.provenance.snapshot_version,
        targets_version: catalog.provenance.targets_version,
        catalog_record_digest: catalog.provenance.catalog_record_digest.clone(),
        archive_digest: catalog.record.archive.sha256.clone(),
    }])
}

pub(super) async fn project_downloads(
    manager: &CognitivePackageManager,
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
) -> UseResult<DownloadProjection> {
    let plan = &envelope.plan;
    if !matches!(
        plan.action,
        PluginOperationAction::Install | PluginOperationAction::Upgrade
    ) {
        return Ok(DownloadProjection::not_required());
    }

    let mut expected = Vec::new();
    let mut expected_planning = Vec::new();
    let mut unavailable = false;
    for transition in &plan.packages {
        if !matches!(
            transition.change,
            PlanPackageChangeKind::Add | PlanPackageChangeKind::Replace
        ) {
            continue;
        }
        let source = transition
            .source
            .as_ref()
            .ok_or_else(diagnostic_state_error)?;
        let PluginPlanSource::Registry {
            provenance,
            archive,
        } = source
        else {
            unavailable = true;
            continue;
        };
        expected.push(ExpectedDownloadTarget {
            package_id: transition.package_id.as_str(),
            provenance,
            archive,
        });
        let locked = envelope
            .package_lock
            .as_ref()
            .and_then(|lock| lock.package(&transition.package_id))
            .ok_or_else(diagnostic_state_error)?;
        if &locked.catalog.provenance != provenance || &locked.catalog.record.archive != archive {
            return Err(diagnostic_state_error());
        }
        if let Some(planning) = locked.catalog.record.planning.as_ref() {
            expected_planning.push(ExpectedPlanningTarget {
                package_id: transition.package_id.as_str(),
                provenance,
                planning,
            });
        }
    }
    let mut projection = observe_expected_downloads(
        manager,
        expected,
        unavailable,
        Some(plan.impact.download_bytes),
    )
    .await?;
    let planning = observe_expected_planning(manager, expected_planning).await?;
    projection.planning_expected_bytes = planning.expected_bytes;
    projection.planning_retained_bytes = planning.retained_bytes;
    projection.planning_status = planning.status;
    projection.planning_targets = planning.targets;
    Ok(projection)
}

pub(super) async fn project_download_attempt(
    manager: &CognitivePackageManager,
    attempt: &PendingPackageDownloadAttempt,
) -> UseResult<DownloadProjection> {
    attempt.validate().map_err(|_| diagnostic_state_error())?;
    let mut expected = Vec::with_capacity(attempt.selected_package_ids.len());
    let mut expected_planning = Vec::new();
    for package in attempt
        .package_lock
        .install_order()
        .map_err(|_| diagnostic_state_error())?
    {
        if !attempt.selected_package_ids.contains(package.package_id()) {
            continue;
        }
        expected.push(ExpectedDownloadTarget {
            package_id: package.package_id(),
            provenance: &package.catalog.provenance,
            archive: &package.catalog.record.archive,
        });
        if let Some(planning) = package.catalog.record.planning.as_ref() {
            expected_planning.push(ExpectedPlanningTarget {
                package_id: package.package_id(),
                provenance: &package.catalog.provenance,
                planning,
            });
        }
    }
    if expected.len() != attempt.selected_package_ids.len() {
        return Err(diagnostic_state_error());
    }
    let mut projection = observe_expected_downloads(manager, expected, false, None).await?;
    let planning = observe_expected_planning(manager, expected_planning).await?;
    projection.planning_expected_bytes = planning.expected_bytes;
    projection.planning_retained_bytes = planning.retained_bytes;
    projection.planning_status = planning.status;
    projection.planning_targets = planning.targets;
    Ok(projection)
}

struct ExpectedDownloadTarget<'a> {
    package_id: &'a str,
    provenance: &'a a3s_use_core::VerifiedCatalogProvenance,
    archive: &'a a3s_use_core::CatalogArchive,
}

struct ExpectedPlanningTarget<'a> {
    package_id: &'a str,
    provenance: &'a a3s_use_core::VerifiedCatalogProvenance,
    planning: &'a a3s_use_core::CatalogPlanningTarget,
}

struct PlanningProjection {
    expected_bytes: u64,
    retained_bytes: u64,
    status: PluginDownloadDiagnosticStatus,
    targets: Vec<PluginPlanningTargetDiagnostic>,
}

async fn observe_expected_downloads(
    manager: &CognitivePackageManager,
    expected: Vec<ExpectedDownloadTarget<'_>>,
    unavailable: bool,
    declared_bytes: Option<u64>,
) -> UseResult<DownloadProjection> {
    if expected.is_empty() {
        return Err(diagnostic_state_error());
    }
    let sources = RegistrySourceStore::new(manager.registry.paths().use_paths().clone());
    let mut targets = Vec::with_capacity(expected.len());
    let mut expected_bytes = 0u64;
    let mut retained_bytes = 0u64;
    for target in expected {
        let observed = sources
            .observe_retained_target(
                target.provenance,
                target.archive.length,
                &target.archive.sha256,
            )
            .await
            .map_err(|_| diagnostic_state_error())?;
        if observed.registry_name != target.provenance.registry_name
            || observed.target_digest != target.archive.sha256
            || observed.expected_bytes != target.archive.length
            || observed.retained_bytes > observed.expected_bytes
        {
            return Err(diagnostic_state_error());
        }
        expected_bytes = expected_bytes
            .checked_add(observed.expected_bytes)
            .ok_or_else(diagnostic_state_error)?;
        retained_bytes = retained_bytes
            .checked_add(observed.retained_bytes)
            .ok_or_else(diagnostic_state_error)?;
        let status = match observed.status {
            a3s_use_extension::VerifiedTargetObservationStatus::Missing => {
                PluginDownloadTargetDiagnosticStatus::Missing
            }
            a3s_use_extension::VerifiedTargetObservationStatus::Partial => {
                PluginDownloadTargetDiagnosticStatus::Partial
            }
            a3s_use_extension::VerifiedTargetObservationStatus::Complete => {
                PluginDownloadTargetDiagnosticStatus::Complete
            }
        };
        targets.push(PluginDownloadTargetDiagnostic {
            package_id: target.package_id.to_owned(),
            registry_name: observed.registry_name,
            archive_digest: observed.target_digest,
            expected_bytes: observed.expected_bytes,
            retained_bytes: observed.retained_bytes,
            status,
        });
    }
    targets.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let declared_mismatch = declared_bytes.is_some_and(|bytes| bytes != expected_bytes);
    let status = if unavailable || declared_mismatch {
        PluginDownloadDiagnosticStatus::Unavailable
    } else if targets
        .iter()
        .all(|target| target.status == PluginDownloadTargetDiagnosticStatus::Complete)
    {
        PluginDownloadDiagnosticStatus::Complete
    } else if targets
        .iter()
        .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Partial)
    {
        PluginDownloadDiagnosticStatus::InProgress
    } else {
        PluginDownloadDiagnosticStatus::Missing
    };
    Ok(DownloadProjection {
        expected_bytes,
        retained_bytes,
        status,
        targets,
        planning_expected_bytes: 0,
        planning_retained_bytes: 0,
        planning_status: PluginDownloadDiagnosticStatus::NotRequired,
        planning_targets: Vec::new(),
    })
}

async fn observe_expected_planning(
    manager: &CognitivePackageManager,
    expected: Vec<ExpectedPlanningTarget<'_>>,
) -> UseResult<PlanningProjection> {
    if expected.is_empty() {
        return Ok(PlanningProjection {
            expected_bytes: 0,
            retained_bytes: 0,
            status: PluginDownloadDiagnosticStatus::NotRequired,
            targets: Vec::new(),
        });
    }
    let sources = RegistrySourceStore::new(manager.registry.paths().use_paths().clone());
    let mut targets = Vec::with_capacity(expected.len());
    let mut expected_bytes = 0u64;
    let mut retained_bytes = 0u64;
    for target in expected {
        let observed = sources
            .observe_retained_target(
                target.provenance,
                target.planning.length,
                &target.planning.sha256,
            )
            .await
            .map_err(|_| diagnostic_state_error())?;
        if observed.registry_name != target.provenance.registry_name
            || observed.target_digest != target.planning.sha256
            || observed.expected_bytes != target.planning.length
            || observed.retained_bytes > observed.expected_bytes
        {
            return Err(diagnostic_state_error());
        }
        expected_bytes = expected_bytes
            .checked_add(observed.expected_bytes)
            .ok_or_else(diagnostic_state_error)?;
        retained_bytes = retained_bytes
            .checked_add(observed.retained_bytes)
            .ok_or_else(diagnostic_state_error)?;
        let status = match observed.status {
            a3s_use_extension::VerifiedTargetObservationStatus::Missing => {
                PluginDownloadTargetDiagnosticStatus::Missing
            }
            a3s_use_extension::VerifiedTargetObservationStatus::Partial => {
                PluginDownloadTargetDiagnosticStatus::Partial
            }
            a3s_use_extension::VerifiedTargetObservationStatus::Complete => {
                PluginDownloadTargetDiagnosticStatus::Complete
            }
        };
        targets.push(PluginPlanningTargetDiagnostic {
            package_id: target.package_id.to_owned(),
            registry_name: observed.registry_name,
            target_digest: observed.target_digest,
            expected_bytes: observed.expected_bytes,
            retained_bytes: observed.retained_bytes,
            status,
        });
    }
    targets.sort_by(|left, right| left.package_id.cmp(&right.package_id));
    let status = if targets
        .iter()
        .all(|target| target.status == PluginDownloadTargetDiagnosticStatus::Complete)
    {
        PluginDownloadDiagnosticStatus::Complete
    } else if targets
        .iter()
        .any(|target| target.status == PluginDownloadTargetDiagnosticStatus::Partial)
    {
        PluginDownloadDiagnosticStatus::InProgress
    } else {
        PluginDownloadDiagnosticStatus::Missing
    };
    Ok(PlanningProjection {
        expected_bytes,
        retained_bytes,
        status,
        targets,
    })
}

pub(super) fn expected_lifecycle_units(
    pending: &PendingPackageGraphOperation,
) -> UseResult<Vec<ExpectedLifecycleUnit>> {
    expected_lifecycle_units_for_envelope(
        &pending.envelope,
        &pending.generations,
        &pending.prior_generations,
    )
}

/// Reconstruct expected lifecycle units from a Host-reviewed plan without the
/// legacy pending-graph leaf. Generations come from the plan's state revision
/// and package-lock install order.
pub(super) fn expected_lifecycle_units_from_envelope(
    envelope: &a3s_use_core::PluginOperationPlanEnvelope,
) -> UseResult<Vec<ExpectedLifecycleUnit>> {
    let generations = reconstructed_install_generations(envelope)?;
    expected_lifecycle_units_for_envelope(envelope, &generations, &BTreeMap::new())
}

include!("projection_lifecycle.rs");
