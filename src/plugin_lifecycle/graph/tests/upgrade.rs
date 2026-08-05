use super::*;

#[tokio::test]
async fn dependency_closure_upgrade_cuts_over_once_then_retires_prior_generations() {
    let fixture = upgrade_graph_fixture();
    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let time = AtomicU64::new(0);

    *fixture.host.fail_once.lock().await = Some("acme/base:remove".to_string());
    let interrupted = graph
        .apply_upgrade(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.plugin.test_injected_failure");

    let records = graph
        .apply_upgrade(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            || time.fetch_add(1, Ordering::Relaxed) + 1,
        )
        .await
        .unwrap();
    assert_eq!(records.len(), 4);
    assert!(records
        .iter()
        .all(|record| record.status == PluginLifecycleOperationStatus::Completed));
    assert_eq!(
        fixture.host.calls.lock().await.as_slice(),
        [
            "acme/base:commit",
            "acme/base:okf-prepare",
            "acme/base:skill-prepare",
            "acme/root:commit",
            "acme/root:okf-prepare",
            "acme/root:skill-prepare",
            "batch:acme/base,acme/root",
            "acme/root:hide",
            "acme/root:drain",
            "acme/root:skill-remove",
            "acme/root:okf-remove",
            "acme/root:remove",
            "acme/base:hide",
            "acme/base:drain",
            "acme/base:skill-remove",
            "acme/base:okf-remove",
            "acme/base:remove",
            "batch:acme/base,acme/root",
            "acme/base:remove",
        ]
    );
}

#[tokio::test]
async fn interrupted_mixed_rollback_states_converge_before_plan_rejection() {
    let fixture = upgrade_graph_fixture();
    for candidate in &fixture.candidates {
        candidate
            .coordinator
            .prepare_for_graph(&candidate.intent, &candidate.manifest, &|| 1)
            .await
            .unwrap();
    }
    fixture.candidates[0]
        .coordinator
        .start_graph_rollback(
            &fixture.candidates[0].intent,
            &fixture.candidates[0].manifest,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.candidates[0]
            .coordinator
            .graph_candidate_status(&fixture.candidates[0].intent)
            .await
            .unwrap(),
        Some(PluginLifecycleOperationStatus::RollingBack)
    );
    assert_eq!(
        fixture.candidates[1]
            .coordinator
            .graph_candidate_status(&fixture.candidates[1].intent)
            .await
            .unwrap(),
        Some(PluginLifecycleOperationStatus::Applying)
    );

    let graph = PluginPackageGraphLifecycleCoordinator::new(fixture.host.clone());
    let error = graph
        .apply_upgrade(
            &fixture.envelope,
            &fixture.prior_lock,
            &fixture.candidates,
            &fixture.retirements,
            || 2,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.package_graph_upgrade_rolled_back");
    for candidate in &fixture.candidates {
        assert_eq!(
            candidate
                .coordinator
                .graph_candidate_status(&candidate.intent)
                .await
                .unwrap(),
            Some(PluginLifecycleOperationStatus::RolledBack)
        );
    }
    let calls = fixture.host.calls.lock().await;
    assert!(calls.iter().all(|call| !call.starts_with("batch:")));
    assert_eq!(
        calls
            .iter()
            .filter(|call| call.ends_with(":candidate-rollback"))
            .count(),
        2
    );
}

#[tokio::test]
async fn failed_upgrade_preparation_rolls_back_ambiguous_surfaces_and_blocks_plan_reuse() {
    let old_catalog = catalog_version("acme/root", Vec::new(), "1.0.0", 'c');
    let old_lock =
        PluginPackageResolver::new(PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap())
            .resolve(old_catalog, Vec::new())
            .unwrap();
    let next_catalog = catalog_version("acme/root", Vec::new(), "1.1.0", 'a');
    let next_lock =
        PluginPackageResolver::new(PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap())
            .resolve(next_catalog, Vec::new())
            .unwrap();
    let transition = next_lock
        .package("acme/root")
        .unwrap()
        .catalog
        .replace_transition(
            &old_lock.package("acme/root").unwrap().catalog,
            PlanPackageRole::Root,
            &[],
            &[],
        )
        .unwrap();
    let plan = PluginOperationPlanDraft::new(
        PluginOperationAction::Upgrade,
        "acme/root",
        "runtime:local",
        vec![transition],
        Vec::new(),
        Vec::new(),
        PlannedOperationImpact {
            download_bytes: next_lock.packages[0].catalog.record.archive.length,
            installed_bytes_after: next_lock.packages[0].catalog.record.package.expanded_bytes,
            reclaimed_bytes: 1,
            drain_required: false,
            retained_data: false,
            okf_changes: Vec::new(),
        },
        PlannedStateEvidence {
            state_revision: 2,
            capability_generation: 2,
            receipt_digest: Some(digest('8')),
        },
    )
    .unwrap()
    .bind(PluginOperationPlanBinding {
        operation_id: "upgrade:acme-root:rollback".to_string(),
        created_at_ms: 1,
        expires_at_ms: 2,
        scope: PlanScope {
            kind: PlanScopeKind::User,
            id: "current".to_string(),
        },
        authority: PlanAuthority {
            actor: PlanActor::User,
            decision: PlanPolicyDecision::Ask,
            policy_digest: digest('9'),
            confirmation_required: true,
        },
    })
    .unwrap();
    let envelope =
        PluginOperationPlanEnvelope::new_with_package_lock(plan, next_lock.clone()).unwrap();
    let next_manifest = manifest_version("acme/root", None, "1.1.0");
    let prior_manifest = manifest_version("acme/root", None, "1.0.0");
    let next_state = envelope.plan.packages[0].after.as_ref().unwrap();
    let prior_state = envelope.plan.packages[0].before.as_ref().unwrap();
    let candidate_intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            scope_id: envelope.plan.scope.id.clone(),
            package_id: "acme/root".to_string(),
            package_digest: next_state.release.package_sha256.clone(),
            manifest_digest: next_state.release.manifest_sha256.clone(),
            generation: 2,
            action: PluginLifecycleAction::Upgrade,
        },
        &next_manifest,
    )
    .unwrap();
    let retirement_intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            scope_id: envelope.plan.scope.id.clone(),
            package_id: "acme/root".to_string(),
            package_digest: prior_state.release.package_sha256.clone(),
            manifest_digest: prior_state.release.manifest_sha256.clone(),
            generation: 1,
            action: PluginLifecycleAction::Uninstall,
        },
        &prior_manifest,
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let host = Arc::new(RecordingHost::default());
    *host.fail_once.lock().await = Some("acme/root:skill-prepare".to_string());
    let candidate = PluginPackageLifecycleUnit::new(
        coordinator(temp.path(), host.clone()),
        candidate_intent,
        next_manifest,
    )
    .unwrap();
    let retirement = PluginPackageLifecycleUnit::new(
        coordinator(temp.path(), host.clone()),
        retirement_intent,
        prior_manifest,
    )
    .unwrap();
    let graph = PluginPackageGraphLifecycleCoordinator::new(host.clone());

    let error = graph
        .apply_upgrade(
            &envelope,
            &old_lock,
            std::slice::from_ref(&candidate),
            std::slice::from_ref(&retirement),
            || 1,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.test_injected_failure");
    assert_eq!(
        candidate
            .coordinator
            .graph_candidate_status(&candidate.intent)
            .await
            .unwrap(),
        Some(PluginLifecycleOperationStatus::RolledBack)
    );
    assert_eq!(
        host.calls.lock().await.as_slice(),
        [
            "acme/root:commit",
            "acme/root:okf-prepare",
            "acme/root:skill-prepare",
            "acme/root:skill-remove",
            "acme/root:okf-remove",
            "acme/root:candidate-rollback",
        ]
    );

    let replay = graph
        .apply_upgrade(
            &envelope,
            &old_lock,
            std::slice::from_ref(&candidate),
            std::slice::from_ref(&retirement),
            || 2,
        )
        .await
        .unwrap_err();
    assert_eq!(replay.code, "use.plugin.package_graph_upgrade_rolled_back");
    assert_eq!(host.calls.lock().await.len(), 6);
}
