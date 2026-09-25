// CognitivePackageManager diagnostic query methods (included into diagnostic).

impl CognitivePackageManager {
    /// Diagnose one exact retained graph or active enablement operation without
    /// applying, recovering, cancelling, reconciling, or otherwise mutating it.
    pub async fn diagnose_operation(
        &self,
        package_id: &str,
    ) -> UseResult<PluginOperationDiagnostic> {
        let parsed_package_id = PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The operation diagnostic package identity is invalid.")
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        // Control-native: never treat `operations/package-graphs` as authority.
        let enablement = pending_enablement(self, &parsed_package_id).await?;
        if let Some(active) = enablement {
            return diagnose_enablement_operation(self, package_id, active).await;
        }
        let reviewed =
            PluginHostProtocolStore::new(self.registry.paths().installation_state_root())
                .get_enablement_diagnostic(self.scope(), &parsed_package_id)
                .await
                .map_err(|_| diagnostic_state_error())?;
        if let Some((record, cancellation)) = reviewed {
            if let Some(diagnostic) = diagnose_reviewed_enablement_operation(
                self,
                &parsed_package_id,
                record,
                cancellation,
            )
            .await?
            {
                return Ok(diagnostic);
            }
        }
        Err(UseError::new(
            "use.plugin.operation_diagnostic_not_found",
            "No diagnosable cognitive-package operation exists for this package and scope.",
        )
        .with_suggestion(
            "Use 'a3s-use extension inspect <publisher/name> --json' for installed lifecycle history.",
        ))
    }

    /// Diagnose an exact retained target set before a reviewed graph exists.
    ///
    /// This projection observes cache state only. The retained package lock
    /// selects historical Registry datastores but is never exposed as apply or
    /// recovery authority.
    pub async fn diagnose_download_attempt(
        &self,
        package_id: &str,
    ) -> UseResult<PluginDownloadAttemptDiagnostic> {
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The download diagnostic package identity is invalid.")
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let attempt = self
            .download_attempt_store()
            .get_for_package(package_id)
            .await
            .map_err(|_| diagnostic_state_error())?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.download_attempt_diagnostic_not_found",
                    "No retained pre-plan package download exists for this package and scope.",
                )
            })?;
        if attempt.scope != *self.scope() || attempt.root_package_id != package_id {
            return Err(diagnostic_state_error());
        }
        let download = project_download_attempt(self, &attempt).await?;
        let diagnostic = PluginDownloadAttemptDiagnostic {
            schema: PLUGIN_DOWNLOAD_ATTEMPT_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: attempt.scope,
            package_id: attempt.root_package_id,
            attempt: PluginPendingDownloadAttemptDiagnostic {
                action: attempt.action,
                phase: PluginDownloadAttemptPhase::PrePlan,
                started_at_ms: attempt.started_at_ms,
                package_lock_digest: attempt.package_lock_digest,
                package_count: bounded_count(
                    attempt.package_lock.packages.len(),
                    "download package",
                )?,
                download_bytes: download.expected_bytes,
                download_retained_bytes: download.retained_bytes,
                download_target_count: bounded_count(download.targets.len(), "download target")?,
                download: download.status,
                downloads: download.targets,
                planning_bytes: download.planning_expected_bytes,
                planning_retained_bytes: download.planning_retained_bytes,
                planning_target_count: bounded_count(
                    download.planning_targets.len(),
                    "planning target",
                )?,
                planning: download.planning_status,
                planning_targets: download.planning_targets,
            },
        };
        diagnostic
            .validate()
            .map_err(|_| diagnostic_state_error())?;
        Ok(diagnostic)
    }

    /// Diagnose the exact Registry/TUF phase before an immutable package lock
    /// exists. Reading never acquires Registry locks, contacts a Registry, or
    /// changes retained planning evidence.
    pub async fn diagnose_resolution_attempt(
        &self,
        package_id: &str,
    ) -> UseResult<PluginResolutionAttemptDiagnostic> {
        PluginPackageId::parse(package_id.to_owned()).map_err(|_| {
            diagnostic_error("The resolution diagnostic package identity is invalid.")
        })?;
        let _maintenance = self
            .maintenance_lock()
            .acquire_shared()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let attempt = self
            .resolution_attempt_store()
            .get_for_package(package_id)
            .await
            .map_err(|_| diagnostic_state_error())?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.resolution_attempt_diagnostic_not_found",
                    "No retained pre-lock Registry resolution exists for this package and scope.",
                )
            })?;
        if attempt.scope != *self.scope() || attempt.root_package_id != package_id {
            return Err(diagnostic_state_error());
        }
        let registries = attempt
            .registries
            .into_iter()
            .map(|registry| PluginRegistryResolutionDiagnostic {
                registry_name: registry.registry_name,
                role: match registry.role {
                    PackageRegistryResolutionRole::Root => PluginRegistryResolutionRole::Root,
                    PackageRegistryResolutionRole::Dependency => {
                        PluginRegistryResolutionRole::Dependency
                    }
                },
                source_identity_digest: registry.source_identity_digest,
                trust_root_digest: registry.trust_root_digest,
                status: match registry.status {
                    PackageRegistryResolutionStatus::Pending => {
                        PluginRegistryResolutionStatus::Pending
                    }
                    PackageRegistryResolutionStatus::Verifying => {
                        PluginRegistryResolutionStatus::Verifying
                    }
                    PackageRegistryResolutionStatus::Verified => {
                        PluginRegistryResolutionStatus::Verified
                    }
                    PackageRegistryResolutionStatus::Failed => {
                        PluginRegistryResolutionStatus::Failed
                    }
                },
                root_version: registry.root_version,
                timestamp_version: registry.timestamp_version,
                snapshot_version: registry.snapshot_version,
                targets_version: registry.targets_version,
                package_targets: registry.package_targets,
                observed_at_ms: registry.observed_at_ms,
                error_code: registry.error_code,
            })
            .collect::<Vec<_>>();
        let diagnostic = PluginResolutionAttemptDiagnostic {
            schema: PLUGIN_RESOLUTION_ATTEMPT_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: attempt.scope,
            package_id: attempt.root_package_id,
            attempt: PluginPendingResolutionAttemptDiagnostic {
                action: attempt.action,
                phase: PluginResolutionAttemptPhase::PreLock,
                access: match attempt.access {
                    PackageResolutionAccess::Refreshed => PluginRegistryResolutionAccess::Refreshed,
                    PackageResolutionAccess::Cached => PluginRegistryResolutionAccess::Cached,
                },
                status: match attempt.status {
                    PackageResolutionAttemptStatus::Resolving => {
                        PluginResolutionDiagnosticStatus::Resolving
                    }
                    PackageResolutionAttemptStatus::Resolved => {
                        PluginResolutionDiagnosticStatus::Resolved
                    }
                    PackageResolutionAttemptStatus::Failed => {
                        PluginResolutionDiagnosticStatus::Failed
                    }
                },
                started_at_ms: attempt.started_at_ms,
                completed_at_ms: attempt.completed_at_ms,
                requested_version: attempt.requested_version,
                channel: attempt.channel,
                registry_count: bounded_count(registries.len(), "resolution Registry")?,
                verified_registry_count: bounded_count(
                    registries
                        .iter()
                        .filter(|registry| {
                            registry.status == PluginRegistryResolutionStatus::Verified
                        })
                        .count(),
                    "verified Registry",
                )?,
                package_lock_digest: attempt.package_lock_digest,
                package_count: attempt.package_count,
                error_code: attempt.error_code,
                registries,
            },
        };
        diagnostic
            .validate()
            .map_err(|_| diagnostic_state_error())?;
        Ok(diagnostic)
    }

    pub(super) async fn diagnose_graph_operation(
        &self,
        package_id: &str,
        pending: PendingPackageGraphOperation,
    ) -> UseResult<PluginOperationDiagnostic> {
        if pending.envelope.plan.scope != *self.scope() {
            return Err(diagnostic_state_error());
        }

        let (generation, snapshot_digest, pending_cutovers) = self
            .control_registry_diagnostic_face()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let expected = expected_lifecycle_units(&pending)?;
        let phase = diagnostic_phase(pending.phase());
        let observed = observe_lifecycle(
            self,
            &pending.envelope.plan.operation_id,
            &pending.envelope.plan_digest,
            phase,
            &expected,
        )
        .await?;
        let grant = observe_grant(self, &pending.envelope, &pending.authorization, phase).await?;
        let cutover_key =
            operation_cutover_key(&pending.envelope).map_err(|_| diagnostic_state_error())?;
        let operation_cutover = project_registry_cutover(
            &pending.envelope,
            phase,
            &cutover_key,
            &pending_cutovers,
            generation,
            &observed,
            &grant,
        )?;
        let registry = PluginRegistryOperationDiagnostic {
            generation,
            snapshot_digest,
            pending_cutover_count: bounded_count(pending_cutovers.len(), "Registry cutover")?,
            operation_cutover,
        };
        let sources = project_sources(&pending.envelope.plan)?;
        let downloads = project_downloads(self, &pending.envelope).await?;
        let providers = project_providers(&pending.envelope.plan, &observed)?;
        let lifecycle = observed
            .iter()
            .map(|unit| unit.summary.clone())
            .collect::<Vec<_>>();
        let recovery = if registry.operation_cutover.status
            == PluginRegistryCutoverDiagnosticStatus::GenerationDrift
        {
            PluginOperationRecoveryGuidance::OperatorReviewRequired
        } else {
            match phase {
                PluginOperationDiagnosticPhase::Planned => {
                    PluginOperationRecoveryGuidance::ReviewAndApplyExactPlan
                }
                PluginOperationDiagnosticPhase::Admitted => {
                    PluginOperationRecoveryGuidance::ResumeExactPlan
                }
                PluginOperationDiagnosticPhase::Cancelled => {
                    PluginOperationRecoveryGuidance::ObserveCancellation
                }
            }
        };
        let operation = pending_operation_diagnostic(
            &pending.envelope,
            phase,
            pending.planned_at_ms,
            (pending.admitted_at_ms > 0).then_some(pending.admitted_at_ms),
            (pending.cancelled_at_ms > 0).then_some(pending.cancelled_at_ms),
            confirmation_status(&pending.envelope, &pending.authorization, phase),
            sources,
            providers,
            grant,
            lifecycle,
            expected.len(),
            downloads,
            recovery,
        )?;
        let diagnostic = PluginOperationDiagnostic {
            schema: PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: self.scope().clone(),
            package_id: package_id.to_owned(),
            registry,
            operation,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }

    /// Project a pre-admission Host cancel without the legacy pending-graph leaf.
    pub(super) async fn diagnose_cancelled_host_graph(
        &self,
        package_id: &str,
        envelope: &PluginOperationPlanEnvelope,
        cancelled_at_ms: u64,
    ) -> UseResult<PluginOperationDiagnostic> {
        if envelope.plan.scope != *self.scope() || envelope.plan.package_id != package_id {
            return Err(diagnostic_state_error());
        }
        let phase = PluginOperationDiagnosticPhase::Cancelled;
        let (generation, snapshot_digest, pending_cutovers) = self
            .control_registry_diagnostic_face()
            .await
            .map_err(|_| diagnostic_state_error())?;
        let expected = expected_lifecycle_units_from_envelope(envelope)?;
        let observed = observe_lifecycle(
            self,
            &envelope.plan.operation_id,
            &envelope.plan_digest,
            phase,
            &expected,
        )
        .await?;
        let required_grant = envelope.plan.workspace_impacts.iter().any(|impact| {
            impact.grant_before_digest.is_some() || impact.grant_after_digest.is_some()
        });
        let grant = empty_grant_diagnostic(required_grant, PluginGrantDiagnosticStatus::Cancelled);
        let cutover_key = operation_cutover_key(envelope).map_err(|_| diagnostic_state_error())?;
        let operation_cutover = project_registry_cutover(
            envelope,
            phase,
            &cutover_key,
            &pending_cutovers,
            generation,
            &observed,
            &grant,
        )?;
        let registry = PluginRegistryOperationDiagnostic {
            generation,
            snapshot_digest,
            pending_cutover_count: bounded_count(pending_cutovers.len(), "Registry cutover")?,
            operation_cutover,
        };
        let sources = project_sources(&envelope.plan)?;
        let downloads = project_downloads(self, envelope).await?;
        let providers = project_providers(&envelope.plan, &observed)?;
        let lifecycle = observed
            .iter()
            .map(|unit| unit.summary.clone())
            .collect::<Vec<_>>();
        let operation = pending_operation_diagnostic(
            envelope,
            phase,
            envelope.plan.created_at_ms,
            None,
            Some(cancelled_at_ms),
            PluginOperationConfirmationDiagnosticStatus::Cancelled,
            sources,
            providers,
            grant,
            lifecycle,
            expected.len(),
            downloads,
            PluginOperationRecoveryGuidance::ObserveCancellation,
        )?;
        let diagnostic = PluginOperationDiagnostic {
            schema: PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA.to_owned(),
            observed_at_ms: super::plan::now_ms().map_err(|_| diagnostic_state_error())?,
            scope: self.scope().clone(),
            package_id: package_id.to_owned(),
            registry,
            operation,
        };
        diagnostic.validate()?;
        Ok(diagnostic)
    }
}

#[allow(clippy::too_many_arguments)]
fn pending_operation_diagnostic(
    envelope: &PluginOperationPlanEnvelope,
    phase: PluginOperationDiagnosticPhase,
    planned_at_ms: u64,
    admitted_at_ms: Option<u64>,
    cancelled_at_ms: Option<u64>,
    confirmation: PluginOperationConfirmationDiagnosticStatus,
    sources: Vec<PluginOperationSourceDiagnostic>,
    providers: Vec<PluginProviderOperationDiagnostic>,
    grant: PluginGrantOperationDiagnostic,
    lifecycle: Vec<PluginLifecycleOperationSummary>,
    lifecycle_unit_count: usize,
    download: DownloadProjection,
    recovery: PluginOperationRecoveryGuidance,
) -> UseResult<PluginPendingOperationDiagnostic> {
    let plan = &envelope.plan;
    let changed_package_count = if matches!(
        plan.action,
        PluginOperationAction::Enable | PluginOperationAction::Disable
    ) {
        plan.packages.len()
    } else {
        plan.packages
            .iter()
            .filter(|package| package.change != PlanPackageChangeKind::Retain)
            .count()
    };
    Ok(PluginPendingOperationDiagnostic {
        operation_id: plan.operation_id.clone(),
        action: plan.action,
        phase,
        plan_digest: envelope.plan_digest.clone(),
        created_at_ms: plan.created_at_ms,
        expires_at_ms: plan.expires_at_ms,
        planned_at_ms,
        admitted_at_ms,
        cancelled_at_ms,
        package_lock_digest: plan.package_lock_digest.clone(),
        prior_package_lock_digest: plan.prior_package_lock_digest.clone(),
        authority_actor: plan.authority.actor,
        authority_decision: plan.authority.decision,
        confirmation,
        package_count: bounded_count(plan.packages.len(), "package")?,
        changed_package_count: bounded_count(changed_package_count, "changed package")?,
        source_count: bounded_count(sources.len(), "source")?,
        provider_count: bounded_count(providers.len(), "provider")?,
        lifecycle_unit_count: bounded_count(lifecycle_unit_count, "lifecycle unit")?,
        observed_lifecycle_unit_count: bounded_count(lifecycle.len(), "observed lifecycle unit")?,
        download_bytes: plan.impact.download_bytes,
        download_retained_bytes: download.retained_bytes,
        download_target_count: bounded_count(download.targets.len(), "download target")?,
        download: download.status,
        plan_drain_required: plan.impact.drain_required,
        downloads: download.targets,
        planning_bytes: download.planning_expected_bytes,
        planning_retained_bytes: download.planning_retained_bytes,
        planning_target_count: bounded_count(download.planning_targets.len(), "planning target")?,
        planning: download.planning_status,
        planning_targets: download.planning_targets,
        sources,
        providers,
        grant,
        lifecycle,
        recovery,
    })
}

pub(super) fn bounded_count(count: usize, kind: &str) -> UseResult<u32> {
    u32::try_from(count).map_err(|_| {
        diagnostic_error(format!(
            "The {kind} diagnostic count exceeds its public bound."
        ))
    })
}

fn valid_sha256(value: &str) -> bool {
    value.strip_prefix("sha256:").is_some_and(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    })
}

fn valid_machine_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        && value.bytes().all(|byte| {
            byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b':' | b'/' | b'@')
        })
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn diagnostic_error(message: impl Into<String>) -> UseError {
    UseError::new("use.plugin.operation_diagnostic_invalid", message)
}

pub(super) fn diagnostic_state_error() -> UseError {
    UseError::new(
        "use.plugin.operation_diagnostic_state_invalid",
        "The retained cognitive-package evidence is unsupported, damaged, or internally inconsistent.",
    )
    .with_suggestion(
        "Preserve the unsupported state for incident review, remove it only with an approved cleanup procedure, then reinstall the package from a trusted Registry.",
    )
}
