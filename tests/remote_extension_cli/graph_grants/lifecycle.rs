use super::*;

#[tokio::test]
async fn permission_grants_follow_install_upgrade_uninstall_and_survive_replay() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v1",
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v2",
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 53, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("home");
    let registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let managed_scope = PlanScope {
        kind: PlanScopeKind::Workspace,
        id: MANAGED_SCOPE_ID.to_string(),
    };
    let extension_registry =
        ExtensionRegistry::new(extension_paths_for(&home, managed_scope.clone()));
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        extension_registry.clone(),
        managed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    assert_eq!(manager.scope(), &managed_scope);

    let registry_lock = exclusive_lock(
        &extension_paths_for(&home, managed_scope.clone())
            .state_root()
            .join("extensions/.registry.lock"),
    );
    let interrupted = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let wrong_scope = PlanScope {
        kind: PlanScopeKind::User,
        id: MANAGED_SCOPE_ID.to_string(),
    };
    let scope_error = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        extension_registry.clone(),
        wrong_scope,
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap_err();
    assert_eq!(scope_error.code, "use.plugin.package_installation_mismatch");
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);

    let pending_path = extension_paths_for(&home, managed_scope.clone())
        .state_root()
        .join("operations/package-graphs/install/acme/worker.json");
    let pending_bytes = std::fs::read(&pending_path).unwrap();
    let pending: serde_json::Value = serde_json::from_slice(&pending_bytes).unwrap();
    assert_eq!(pending["envelope"]["plan"]["scope"]["kind"], "workspace");
    assert_eq!(pending["envelope"]["plan"]["scope"]["id"], MANAGED_SCOPE_ID);
    let mut tampered = Vec::new();

    let mut missing_resolved = pending.clone();
    missing_resolved["authorization"]
        .as_object_mut()
        .unwrap()
        .remove("resolvedGrants");
    tampered.push((
        "missing resolved Grant",
        "use.plugin.package_authorization_invalid",
        missing_resolved,
    ));

    let mut changed_confirmation = pending.clone();
    let confirmed_at = changed_confirmation["authorization"]["operationConfirmation"]
        ["confirmedAtMs"]
        .as_u64()
        .unwrap();
    changed_confirmation["authorization"]["operationConfirmation"]["confirmedAtMs"] =
        serde_json::json!(confirmed_at + 1);
    tampered.push((
        "changed operation confirmation",
        "use.plugin.plan_confirmation_mismatch",
        changed_confirmation,
    ));

    let mut changed_snapshot = pending.clone();
    changed_snapshot["authorization"]["grantSnapshot"]["stateRevision"] = serde_json::json!(999);
    tampered.push((
        "changed Grant snapshot",
        "use.plugin.package_authorization_invalid",
        changed_snapshot,
    ));

    let mut changed_change_set = pending.clone();
    changed_change_set["authorization"]["grantChangeSet"]["stateRevision"] = serde_json::json!(999);
    tampered.push((
        "changed Grant change set",
        "use.plugin.grant_changes_plan_mismatch",
        changed_change_set,
    ));

    let mut changed_ceiling = pending.clone();
    changed_ceiling["authorization"]["grantCeilings"][0]["packageDigest"] =
        serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    tampered.push((
        "changed signed ceiling",
        "use.plugin.package_authorization_invalid",
        changed_ceiling,
    ));

    let mut legacy_permission_operation = pending;
    legacy_permission_operation["schema"] =
        serde_json::json!("a3s.use.pending-package-graph-operation.v1");
    tampered.push((
        "permission-bearing legacy pending schema",
        "use.plugin.package_graph_store_invalid",
        legacy_permission_operation,
    ));

    for (case, expected_code, value) in tampered {
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
        let error = manager
            .install_remote(
                &registry,
                &[],
                "acme/worker",
                Some("1.0.0"),
                PluginReleaseChannel::Stable,
                None,
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, expected_code, "unexpected error for {case}");
        assert_eq!(
            authorization_count.load(Ordering::SeqCst),
            1,
            "tampered pending evidence must not trigger reauthorization: {case}"
        );
    }
    std::fs::write(&pending_path, &pending_bytes).unwrap();

    let installed = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(installed.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    let install_plan = installed.plan.as_ref().unwrap();
    assert_eq!(install_plan.plan.authority, test_authority());
    assert_eq!(install_plan.plan.scope, managed_scope);
    assert_eq!(install_plan.plan.workspace_impacts.len(), 1);
    let first_state = install_plan.plan.packages[0].after.as_ref().unwrap();
    assert_granted(
        &home,
        &managed_scope,
        &first_state.release.package_sha256,
        &first_state.permissions,
    )
    .await;

    let upgrade_lock = resolve_remote_package_lock(
        &registry,
        &[],
        "acme/worker",
        Some("2.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let upgrade_lock_digest = upgrade_lock.descriptor_digest().unwrap();
    let prepared_upgrade = manager
        .prepare_upgrade_remote(
            &registry,
            &[],
            "acme/worker",
            Some("2.0.0"),
            PluginReleaseChannel::Stable,
            &upgrade_lock_digest,
        )
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        manager
            .installed_package_lock("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        installed.package_lock_digest
    );
    assert_eq!(
        manager
            .prepare_upgrade_remote(
                &registry,
                &[],
                "acme/worker",
                Some("2.0.0"),
                PluginReleaseChannel::Stable,
                &upgrade_lock_digest,
            )
            .await
            .unwrap(),
        prepared_upgrade
    );
    let upgraded = manager
        .upgrade_remote(
            &registry,
            &[],
            "acme/worker",
            Some("2.0.0"),
            PluginReleaseChannel::Stable,
            Some(&upgrade_lock_digest),
        )
        .await
        .unwrap();
    assert!(upgraded.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 2);
    let upgrade_plan = upgraded.plan.as_ref().unwrap();
    assert_eq!(upgrade_plan, &prepared_upgrade);
    assert_eq!(upgrade_plan.plan.scope, managed_scope);
    let transition = &upgrade_plan.plan.packages[0];
    let prior = transition.before.as_ref().unwrap();
    let candidate = transition.after.as_ref().unwrap();
    assert_revoked(&home, &managed_scope, &prior.release.package_sha256).await;
    assert_granted(
        &home,
        &managed_scope,
        &candidate.release.package_sha256,
        &candidate.permissions,
    )
    .await;

    let uninstall_lock_digest = upgraded.package_lock_digest.clone();
    let prepared_uninstall = manager
        .prepare_uninstall("acme/worker", &uninstall_lock_digest)
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 2);
    assert_eq!(
        manager
            .installed_package_lock("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        uninstall_lock_digest
    );
    assert_granted(
        &home,
        &managed_scope,
        &candidate.release.package_sha256,
        &candidate.permissions,
    )
    .await;
    assert_eq!(
        manager
            .prepare_uninstall("acme/worker", &uninstall_lock_digest)
            .await
            .unwrap(),
        prepared_uninstall
    );
    let uninstalled = manager.uninstall("acme/worker").await.unwrap();
    assert!(uninstalled.changed);
    assert_eq!(uninstalled.plan, prepared_uninstall);
    assert_eq!(uninstalled.plan.plan.scope, managed_scope);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 3);
    assert_revoked(&home, &managed_scope, &candidate.release.package_sha256).await;
    let managed_state = extension_paths_for(&home, managed_scope.clone()).installation_state_root();
    assert!(!managed_state
        .join("operations/package-graphs/install/acme/worker.json")
        .exists());
    assert!(!managed_state
        .join("operations/package-graphs/upgrade/acme/worker.json")
        .exists());
    assert!(!managed_state
        .join("operations/package-graphs/uninstall/acme/worker.json")
        .exists());
}

#[test]
fn permission_bearing_enablement_cuts_over_grants_and_recovers_after_cutover() {
    const TEST_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

    std::thread::Builder::new()
        .name("permission-bearing-enablement".to_string())
        .stack_size(TEST_THREAD_STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(permission_bearing_enablement_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn permission_bearing_enablement_scenario() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let targets = cognitive_tool_targets_version(
        temporary.path(),
        "acme/worker",
        "worker-v1",
        "1.0.0",
        &target,
    );
    let repository = TestRepository::with_targets(targets, 59, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("home");
    let registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let extension_registry = ExtensionRegistry::new(extension_paths(&home));
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let manager = CognitivePackageManager::with_authorization(
        extension_registry.clone(),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    let installed = extension_registry
        .get("acme/worker")
        .await
        .unwrap()
        .unwrap();
    let lifecycle_generation = installed.receipt.lifecycle_generation.unwrap();
    assert!(installed.receipt.planning_bundle.is_some());
    assert!(installed.plan_ready_planning_bundle().unwrap().is_some());
    let install_plan = manager
        .install_remote(
            &registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(!install_plan.changed);
    let catalog = installed.plan_ready_catalog().unwrap();
    let package_digest = catalog.record.package.sha256.clone().unwrap();
    let permissions = catalog.record.permission_ceiling.clone();
    assert_granted(&home, manager.scope(), &package_digest, &permissions).await;

    let state = manager.observe_package("acme/worker").await.unwrap();
    let request = CognitivePackageEnablementRequest::new(
        "enablement:worker:disable:0001",
        "acme/worker",
        state.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let planned = manager.plan_enablement(&request).await.unwrap();
    assert_eq!(
        planned.status,
        CognitivePackageEnablementPlanStatus::Planned
    );
    let planned_envelope = planned.plan.as_ref().unwrap();
    assert_eq!(planned_envelope.plan.operation_id, request.operation_id);
    assert_eq!(planned_envelope.plan.action, PluginOperationAction::Disable);
    assert_eq!(
        planned_envelope.plan.schema,
        a3s_use_core::PLUGIN_OPERATION_PLAN_SCHEMA_V4
    );
    assert_eq!(
        planned_envelope.plan.authority.decision,
        PlanPolicyDecision::Ask
    );
    assert!(planned_envelope.plan.authority.confirmation_required);
    assert!(planned.result.is_none());
    assert_eq!(planned.state, state);
    assert!(
        extension_registry
            .get("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );
    assert_granted(&home, manager.scope(), &package_digest, &permissions).await;

    let confirmation_required = manager
        .apply_enablement(&request, planned_envelope.clone(), None)
        .await
        .unwrap_err();
    assert_eq!(
        confirmation_required.code,
        "use.plugin.package_reviewed_authorization_invalid"
    );
    assert!(
        extension_registry
            .get("acme/worker")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );
    assert_granted(&home, manager.scope(), &package_digest, &permissions).await;
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);

    let route_lock = exclusive_lock(
        &extension_paths(&home)
            .state_root()
            .join("route-locks/acme/worker")
            .join(format!("{lifecycle_generation:020}.lock")),
    );
    let interrupted_manager = manager.clone();
    let interrupted_request = request.clone();
    let interrupted_plan = planned_envelope.clone();
    let interrupted_confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: interrupted_plan.plan.operation_id.clone(),
        plan_digest: interrupted_plan.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: interrupted_plan.plan.created_at_ms + 1,
    };
    let interrupted = tokio::spawn(async move {
        interrupted_manager
            .apply_enablement(
                &interrupted_request,
                interrupted_plan,
                Some(interrupted_confirmation),
            )
            .await
    });

    let grant_store = WorkspaceGrantStore::new(extension_paths(&home).installation_state_root());
    let mut reached_cutover_drain = false;
    let mut disable_cutover_generation = None;
    for _ in 0..500 {
        let hidden = extension_registry
            .get("acme/worker")
            .await
            .unwrap()
            .is_some_and(|extension| !extension.receipt.enabled);
        let cutover_committed = grant_store
            .observe_change_set(&request.operation_id)
            .await
            .unwrap()
            .is_some_and(|journal| journal.phase == WorkspaceGrantLifecyclePhase::CutoverCommitted);
        if hidden && cutover_committed {
            reached_cutover_drain = true;
            disable_cutover_generation =
                Some(extension_registry.snapshot().await.unwrap().generation);
            break;
        }
        if interrupted.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    if !reached_cutover_drain {
        FileExt::unlock(&route_lock).unwrap();
        drop(route_lock);
        let outcome = interrupted.await;
        panic!("disable did not reach the cutover-before-drain checkpoint: {outcome:?}");
    }
    assert_granted(&home, manager.scope(), &package_digest, &permissions).await;
    assert!(extension_registry
        .find_published_route("worker-v1")
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        extension_registry
            .snapshot()
            .await
            .unwrap()
            .pending_cutovers
            .len(),
        1
    );

    interrupted.abort();
    let _ = interrupted.await;
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);

    let restarted = CognitivePackageManager::with_authorization(
        extension_registry.clone(),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let disabled = apply_planned_enablement(&restarted, &request)
        .await
        .unwrap();
    assert!(disabled.changed);
    assert!(!disabled.replayed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        extension_registry.snapshot().await.unwrap().generation,
        disable_cutover_generation.unwrap()
    );
    assert!(extension_registry
        .snapshot()
        .await
        .unwrap()
        .pending_cutovers
        .is_empty());
    assert_revoked(&home, restarted.scope(), &package_digest).await;
    assert_eq!(
        grant_store
            .observe_change_set(&request.operation_id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        WorkspaceGrantLifecyclePhase::Completed
    );

    let replayed = apply_planned_enablement(&restarted, &request)
        .await
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);

    let enable = CognitivePackageEnablementRequest::new(
        "enablement:worker:enable:0002",
        "acme/worker",
        disabled.state.package_generation.unwrap(),
        true,
    )
    .unwrap();
    let prepared = restarted.prepare_enablement(&enable).await.unwrap();
    let CognitivePackageEnablementPreparation::Draft(prepared) = prepared else {
        panic!("re-enable must produce a provider-neutral draft");
    };
    assert!(prepared.planning_bundles.contains_key("acme/worker"));
    assert_eq!(
        prepared.installed_generations.get("acme/worker"),
        Some(&lifecycle_generation)
    );
    let registry_lock = exclusive_lock(&scoped_state(&home, "extensions/.registry.lock"));
    assert_eq!(
        apply_planned_enablement(&restarted, &enable)
            .await
            .unwrap_err()
            .code,
        "use.extension.busy"
    );
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        grant_store
            .observe_change_set(&enable.operation_id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        WorkspaceGrantLifecyclePhase::Prepared
    );
    assert!(extension_registry
        .find_published_route("worker-v1")
        .await
        .unwrap()
        .is_none());
    assert_granted(&home, restarted.scope(), &package_digest, &permissions).await;
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let enabled = apply_planned_enablement(&restarted, &enable).await.unwrap();
    assert!(enabled.changed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        extension_registry.snapshot().await.unwrap().generation,
        disable_cutover_generation.unwrap() + 1
    );
    assert!(extension_registry
        .snapshot()
        .await
        .unwrap()
        .pending_cutovers
        .is_empty());
    assert_eq!(
        grant_store
            .observe_change_set(&enable.operation_id)
            .await
            .unwrap()
            .unwrap()
            .phase,
        WorkspaceGrantLifecyclePhase::Completed
    );
    assert_granted(&home, restarted.scope(), &package_digest, &permissions).await;
    assert!(extension_registry
        .find_published_route("worker-v1")
        .await
        .unwrap()
        .is_some());
}
