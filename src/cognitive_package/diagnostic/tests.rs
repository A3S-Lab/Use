use super::*;
use a3s_use_core::PlanScopeKind;

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn download_attempt_diagnostic() -> PluginDownloadAttemptDiagnostic {
    PluginDownloadAttemptDiagnostic {
        schema: PLUGIN_DOWNLOAD_ATTEMPT_DIAGNOSTIC_SCHEMA.to_owned(),
        observed_at_ms: 20,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "user/current".to_owned(),
        },
        package_id: "acme/root".to_owned(),
        attempt: PluginPendingDownloadAttemptDiagnostic {
            action: PluginOperationAction::Install,
            phase: PluginDownloadAttemptPhase::PrePlan,
            started_at_ms: 10,
            package_lock_digest: digest('a'),
            package_count: 1,
            download_bytes: 42,
            download_retained_bytes: 20,
            download_target_count: 1,
            download: PluginDownloadDiagnosticStatus::InProgress,
            downloads: vec![PluginDownloadTargetDiagnostic {
                package_id: "acme/root".to_owned(),
                registry_name: "fixture".to_owned(),
                archive_digest: digest('b'),
                expected_bytes: 42,
                retained_bytes: 20,
                status: PluginDownloadTargetDiagnosticStatus::Partial,
            }],
            planning_bytes: 12,
            planning_retained_bytes: 6,
            planning_target_count: 1,
            planning: PluginDownloadDiagnosticStatus::InProgress,
            planning_targets: vec![PluginPlanningTargetDiagnostic {
                package_id: "acme/root".to_owned(),
                registry_name: "fixture".to_owned(),
                target_digest: digest('c'),
                expected_bytes: 12,
                retained_bytes: 6,
                status: PluginDownloadTargetDiagnosticStatus::Partial,
            }],
        },
    }
}

fn resolution_attempt_diagnostic() -> PluginResolutionAttemptDiagnostic {
    PluginResolutionAttemptDiagnostic {
        schema: PLUGIN_RESOLUTION_ATTEMPT_DIAGNOSTIC_SCHEMA.to_owned(),
        observed_at_ms: 20,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "user/current".to_owned(),
        },
        package_id: "acme/root".to_owned(),
        attempt: PluginPendingResolutionAttemptDiagnostic {
            action: PluginOperationAction::Install,
            phase: PluginResolutionAttemptPhase::PreLock,
            access: PluginRegistryResolutionAccess::Refreshed,
            status: PluginResolutionDiagnosticStatus::Resolving,
            started_at_ms: 10,
            completed_at_ms: None,
            requested_version: Some("1.0.0".to_owned()),
            channel: a3s_use_core::PluginReleaseChannel::Stable,
            registry_count: 1,
            verified_registry_count: 0,
            package_lock_digest: None,
            package_count: None,
            error_code: None,
            registries: vec![PluginRegistryResolutionDiagnostic {
                registry_name: "fixture".to_owned(),
                role: PluginRegistryResolutionRole::Root,
                source_identity_digest: digest('9'),
                trust_root_digest: digest('a'),
                status: PluginRegistryResolutionStatus::Verifying,
                root_version: None,
                timestamp_version: None,
                snapshot_version: None,
                targets_version: None,
                package_targets: None,
                observed_at_ms: Some(11),
                error_code: None,
            }],
        },
    }
}

pub(in crate::cognitive_package) fn diagnostic_with_final_pending_checkpoint(
) -> PluginOperationDiagnostic {
    PluginOperationDiagnostic {
        schema: PLUGIN_OPERATION_DIAGNOSTIC_SCHEMA.to_owned(),
        observed_at_ms: 20,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "user/current".to_owned(),
        },
        package_id: "acme/root".to_owned(),
        registry: PluginRegistryOperationDiagnostic {
            generation: 7,
            snapshot_digest: digest('a'),
            pending_cutover_count: 0,
            operation_cutover: PluginRegistryCutoverDiagnostic {
                status: PluginRegistryCutoverDiagnosticStatus::NotObserved,
                expected_generation_before: 7,
                expected_generation_after: 8,
                recorded_generation_after: None,
                recorded_snapshot_digest: None,
            },
        },
        operation: PluginPendingOperationDiagnostic {
            operation_id: "install:acme-root:0001".to_owned(),
            action: PluginOperationAction::Install,
            phase: PluginOperationDiagnosticPhase::Admitted,
            plan_digest: digest('b'),
            created_at_ms: 1,
            expires_at_ms: 100,
            planned_at_ms: 2,
            admitted_at_ms: Some(3),
            cancelled_at_ms: None,
            package_lock_digest: Some(digest('c')),
            prior_package_lock_digest: None,
            authority_actor: PlanActor::User,
            authority_decision: PlanPolicyDecision::Allow,
            confirmation: PluginOperationConfirmationDiagnosticStatus::NotRequired,
            package_count: 1,
            changed_package_count: 1,
            source_count: 1,
            provider_count: 0,
            lifecycle_unit_count: 1,
            observed_lifecycle_unit_count: 1,
            download_bytes: 42,
            download_retained_bytes: 42,
            download_target_count: 1,
            download: PluginDownloadDiagnosticStatus::Complete,
            plan_drain_required: false,
            downloads: vec![PluginDownloadTargetDiagnostic {
                package_id: "acme/root".to_owned(),
                registry_name: "fixture".to_owned(),
                archive_digest: digest('e'),
                expected_bytes: 42,
                retained_bytes: 42,
                status: PluginDownloadTargetDiagnosticStatus::Complete,
            }],
            planning_bytes: 12,
            planning_retained_bytes: 12,
            planning_target_count: 1,
            planning: PluginDownloadDiagnosticStatus::Complete,
            planning_targets: vec![PluginPlanningTargetDiagnostic {
                package_id: "acme/root".to_owned(),
                registry_name: "fixture".to_owned(),
                target_digest: digest('1'),
                expected_bytes: 12,
                retained_bytes: 12,
                status: PluginDownloadTargetDiagnosticStatus::Complete,
            }],
            sources: vec![PluginOperationSourceDiagnostic::Registry {
                package_id: "acme/root".to_owned(),
                registry_name: "fixture".to_owned(),
                root_version: 1,
                timestamp_version: 2,
                snapshot_version: 3,
                targets_version: 4,
                catalog_record_digest: digest('f'),
                archive_digest: digest('e'),
            }],
            providers: Vec::new(),
            grant: PluginGrantOperationDiagnostic {
                required: false,
                status: PluginGrantDiagnosticStatus::NotRequired,
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
            },
            lifecycle: vec![PluginLifecycleOperationSummary {
                package_id: "acme/root".to_owned(),
                action: PluginLifecycleAction::Install,
                status: PluginLifecycleOperationStatus::Applying,
                generation: 1,
                intent_digest: digest('d'),
                completed_checkpoints: 2,
                total_checkpoints: 3,
                publication: PluginLifecyclePublicationDiagnosticStatus::Pending,
                drain: PluginLifecycleDrainDiagnosticStatus::NotRequired,
                current_checkpoint: Some(PluginLifecycleCheckpointDiagnostic {
                    sequence: 3,
                    kind: PluginLifecycleCheckpointKind::CapabilityPublished,
                    surface: None,
                    required: true,
                    status: PluginLifecycleCheckpointDiagnosticStatus::Pending,
                    evidence_digest: None,
                    error_code: None,
                    observed_at_ms: None,
                }),
                rollback_evidence_digest: None,
                completed_at_ms: None,
            }],
            recovery: PluginOperationRecoveryGuidance::ResumeExactPlan,
        },
    }
}

pub(in crate::cognitive_package) fn completed_operation_diagnostic() -> PluginOperationDiagnostic {
    let mut diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.registry.generation = 8;
    diagnostic.registry.operation_cutover.status =
        PluginRegistryCutoverDiagnosticStatus::Acknowledged;
    diagnostic
        .registry
        .operation_cutover
        .recorded_generation_after = Some(8);
    diagnostic
        .registry
        .operation_cutover
        .recorded_snapshot_digest = Some(digest('a'));
    diagnostic.operation.lifecycle[0].status = PluginLifecycleOperationStatus::Completed;
    diagnostic.operation.lifecycle[0].completed_checkpoints = 3;
    diagnostic.operation.lifecycle[0].publication =
        PluginLifecyclePublicationDiagnosticStatus::Published;
    diagnostic.operation.lifecycle[0].current_checkpoint = None;
    diagnostic.operation.lifecycle[0].completed_at_ms = Some(19);
    diagnostic.validate().unwrap();
    diagnostic
}

fn operation_history_diagnostic() -> PluginOperationHistoryDiagnostic {
    let older = completed_operation_diagnostic();
    let mut newer = older.clone();
    newer.observed_at_ms = 21;
    newer.operation.operation_id = "install:acme-root:0002".to_owned();
    newer.operation.plan_digest = digest('9');
    PluginOperationHistoryDiagnostic {
        schema: PLUGIN_OPERATION_HISTORY_DIAGNOSTIC_SCHEMA.to_owned(),
        observed_at_ms: 30,
        scope: older.scope.clone(),
        package_id: older.package_id.clone(),
        retention_limit: MAX_RETAINED_PLUGIN_OPERATION_DIAGNOSTICS as u32,
        retention_byte_limit: MAX_RETAINED_PLUGIN_OPERATION_HISTORY_BYTES as u64,
        retained_operation_count: 2,
        operations: vec![
            PluginRetainedOperationDiagnostic {
                retained_at_ms: newer.observed_at_ms,
                outcome: PluginRetainedOperationOutcome::Completed,
                diagnostic: newer,
            },
            PluginRetainedOperationDiagnostic {
                retained_at_ms: older.observed_at_ms,
                outcome: PluginRetainedOperationOutcome::Completed,
                diagnostic: older,
            },
        ],
    }
}

#[test]
fn contract_round_trips_a_one_based_final_pending_checkpoint() {
    let diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.validate().unwrap();

    let encoded = serde_json::to_vec(&diagnostic).unwrap();
    assert_eq!(
        PluginOperationDiagnostic::from_json(&encoded).unwrap(),
        diagnostic
    );
}

#[test]
fn operation_history_contract_round_trips_newest_first_entries() {
    let diagnostic = operation_history_diagnostic();
    diagnostic.validate().unwrap();

    let encoded = serde_json::to_vec(&diagnostic).unwrap();
    assert_eq!(
        PluginOperationHistoryDiagnostic::from_json(&encoded).unwrap(),
        diagnostic
    );
}

#[test]
fn operation_history_contract_rejects_unknown_fields_and_duplicate_operations() {
    let mut value = serde_json::to_value(operation_history_diagnostic()).unwrap();
    value.as_object_mut().unwrap().insert(
        "credential".to_owned(),
        serde_json::json!("history-secret-sentinel"),
    );
    assert_eq!(
        PluginOperationHistoryDiagnostic::from_json(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = operation_history_diagnostic();
    diagnostic.operations[0].diagnostic.operation.operation_id = diagnostic.operations[1]
        .diagnostic
        .operation
        .operation_id
        .clone();
    diagnostic.operations[0].diagnostic.operation.plan_digest = diagnostic.operations[1]
        .diagnostic
        .operation
        .plan_digest
        .clone();
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = operation_history_diagnostic();
    diagnostic.operations[0].outcome = PluginRetainedOperationOutcome::RolledBack;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn contract_rejects_zero_and_out_of_range_checkpoint_sequences() {
    let mut diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.operation.lifecycle[0]
        .current_checkpoint
        .as_mut()
        .unwrap()
        .sequence = 0;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    diagnostic.operation.lifecycle[0]
        .current_checkpoint
        .as_mut()
        .unwrap()
        .sequence = 4;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn contract_rejects_unknown_fields() {
    let mut value = serde_json::to_value(diagnostic_with_final_pending_checkpoint()).unwrap();
    value.as_object_mut().unwrap().insert(
        "credential".to_owned(),
        serde_json::json!("must-not-be-accepted"),
    );
    let encoded = serde_json::to_vec(&value).unwrap();

    assert_eq!(
        PluginOperationDiagnostic::from_json(&encoded)
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn contract_rejects_inputs_above_the_public_byte_bound() {
    let oversized = vec![b' '; MAX_PLUGIN_OPERATION_DIAGNOSTIC_BYTES + 1];

    assert_eq!(
        PluginOperationDiagnostic::from_json(&oversized)
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );
    assert_eq!(
        PluginDownloadAttemptDiagnostic::from_json(&oversized)
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let oversized_history = vec![b' '; MAX_PLUGIN_OPERATION_HISTORY_BYTES + 1];
    assert_eq!(
        PluginOperationHistoryDiagnostic::from_json(&oversized_history)
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn contract_rejects_inconsistent_download_progress() {
    let mut diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.operation.download_retained_bytes = 41;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.operation.downloads[0].status = PluginDownloadTargetDiagnosticStatus::Partial;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.operation.download = PluginDownloadDiagnosticStatus::Missing;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn contract_accepts_enablement_awaiting_cutover_after_all_checkpoints() {
    let mut diagnostic = diagnostic_with_final_pending_checkpoint();
    diagnostic.operation.action = PluginOperationAction::Enable;
    diagnostic.operation.package_lock_digest = None;
    diagnostic.operation.download_bytes = 0;
    diagnostic.operation.download_retained_bytes = 0;
    diagnostic.operation.download_target_count = 0;
    diagnostic.operation.download = PluginDownloadDiagnosticStatus::NotRequired;
    diagnostic.operation.downloads.clear();
    diagnostic.operation.planning_bytes = 0;
    diagnostic.operation.planning_retained_bytes = 0;
    diagnostic.operation.planning_target_count = 0;
    diagnostic.operation.planning = PluginDownloadDiagnosticStatus::NotRequired;
    diagnostic.operation.planning_targets.clear();
    diagnostic.operation.lifecycle[0].action = PluginLifecycleAction::Enable;
    diagnostic.operation.lifecycle[0].completed_checkpoints = 3;
    diagnostic.operation.lifecycle[0].publication =
        PluginLifecyclePublicationDiagnosticStatus::Published;
    diagnostic.operation.lifecycle[0].current_checkpoint = None;

    diagnostic.validate().unwrap();
    let encoded = serde_json::to_vec(&diagnostic).unwrap();
    assert_eq!(
        PluginOperationDiagnostic::from_json(&encoded).unwrap(),
        diagnostic
    );

    diagnostic.operation.package_lock_digest = Some(digest('e'));
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn download_attempt_contract_round_trips_exact_partial_bytes() {
    let diagnostic = download_attempt_diagnostic();
    diagnostic.validate().unwrap();

    let encoded = serde_json::to_vec(&diagnostic).unwrap();
    assert_eq!(
        PluginDownloadAttemptDiagnostic::from_json(&encoded).unwrap(),
        diagnostic
    );
}

#[test]
fn download_attempt_contract_rejects_unknown_or_inconsistent_evidence() {
    let mut value = serde_json::to_value(download_attempt_diagnostic()).unwrap();
    value.as_object_mut().unwrap().insert(
        "credential".to_owned(),
        serde_json::json!("must-not-be-accepted"),
    );
    assert_eq!(
        PluginDownloadAttemptDiagnostic::from_json(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = download_attempt_diagnostic();
    diagnostic.attempt.download_retained_bytes = 19;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = download_attempt_diagnostic();
    diagnostic.attempt.download = PluginDownloadDiagnosticStatus::Complete;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = download_attempt_diagnostic();
    diagnostic.attempt.planning_retained_bytes = 5;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = download_attempt_diagnostic();
    diagnostic.attempt.planning = PluginDownloadDiagnosticStatus::Complete;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );
}

#[test]
fn resolution_attempt_contract_round_trips_active_and_terminal_states() {
    let active = resolution_attempt_diagnostic();
    active.validate().unwrap();
    let encoded = serde_json::to_vec(&active).unwrap();
    assert_eq!(
        PluginResolutionAttemptDiagnostic::from_json(&encoded).unwrap(),
        active
    );

    let mut failed = resolution_attempt_diagnostic();
    failed.attempt.status = PluginResolutionDiagnosticStatus::Failed;
    failed.attempt.completed_at_ms = Some(19);
    failed.attempt.error_code = Some("use.extension.registry_untrusted".to_owned());
    failed.attempt.registries[0].status = PluginRegistryResolutionStatus::Failed;
    failed.attempt.registries[0].observed_at_ms = Some(19);
    failed.attempt.registries[0].error_code = Some("use.extension.registry_untrusted".to_owned());
    failed.validate().unwrap();

    let mut resolved = resolution_attempt_diagnostic();
    resolved.attempt.status = PluginResolutionDiagnosticStatus::Resolved;
    resolved.attempt.completed_at_ms = Some(19);
    resolved.attempt.package_lock_digest = Some(digest('b'));
    resolved.attempt.package_count = Some(1);
    resolved.attempt.verified_registry_count = 1;
    let registry = &mut resolved.attempt.registries[0];
    registry.status = PluginRegistryResolutionStatus::Verified;
    registry.root_version = Some(1);
    registry.timestamp_version = Some(2);
    registry.snapshot_version = Some(3);
    registry.targets_version = Some(4);
    registry.package_targets = Some(1);
    registry.observed_at_ms = Some(18);
    resolved.validate().unwrap();
}

#[test]
fn resolution_attempt_contract_rejects_unknown_or_inconsistent_evidence() {
    let mut value = serde_json::to_value(resolution_attempt_diagnostic()).unwrap();
    value.as_object_mut().unwrap().insert(
        "registryUrl".to_owned(),
        serde_json::json!("https://secret.example.test/?token=sentinel"),
    );
    assert_eq!(
        PluginResolutionAttemptDiagnostic::from_json(&serde_json::to_vec(&value).unwrap())
            .unwrap_err()
            .code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = resolution_attempt_diagnostic();
    diagnostic.attempt.verified_registry_count = 1;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );

    let mut diagnostic = resolution_attempt_diagnostic();
    diagnostic.attempt.status = PluginResolutionDiagnosticStatus::Resolved;
    assert_eq!(
        diagnostic.validate().unwrap_err().code,
        "use.plugin.operation_diagnostic_invalid"
    );
}
