use super::*;

#[tokio::test]
async fn reviewed_host_install_and_upgrade_apply_use_only_planning_cache() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        &temporary.path().join("v1"),
        "acme/worker",
        "worker-host",
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        &temporary.path().join("v2"),
        "acme/worker",
        "worker-host",
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 117, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("offline-apply-host-home");
    let paths = ExtensionPaths::new(home.join("data"), home.join("state"));
    let sources = RegistrySourceStore::new(paths.clone());
    sources
        .add(RegistrySourceInput::new(
            "fixture",
            server.base_url(),
            &repository.root_sha256,
            None,
            VerifiedTargetCachePolicy::default(),
        ))
        .await
        .unwrap();
    let resolved = sources.resolve(Some("fixture")).await.unwrap();
    let first_lock = resolve_remote_package_lock(
        resolved.root(),
        resolved.dependencies(),
        "acme/worker",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target.clone(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let first_candidate = first_lock.package("acme/worker").unwrap().catalog.clone();
    let managed_scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:offline-apply".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_owned(),
        authority_id: "cloud:control-plane".to_owned(),
        fence_generation: 11,
        fence_digest: format!("sha256:{}", "b".repeat(64)),
    };
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let host = CognitivePackageHostManager::new(
        managed_scope.clone(),
        "use:offline-apply-test",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let package_id = PluginPackageId::parse("acme/worker").unwrap();
    let selected_surfaces = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_owned(),
    }];

    let install_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:offline-install:0001".to_owned(),
        assignment_generation: 11,
        capabilities_digest: capabilities_digest.clone(),
        scope: managed_scope.clone(),
        action: PluginOperationAction::Install,
        package_id: package_id.clone(),
        candidate: Some(first_candidate),
        package_lock: Some(first_lock),
        selected_surfaces: selected_surfaces.clone(),
    };
    let install_plan = host.plan(install_request.clone()).await.unwrap();
    let install_apply = apply_request(
        &install_request,
        &install_plan,
        "apply:offline-install:0001",
    );
    server.clear_requests();
    let installed = host.apply(install_apply).await.unwrap();
    assert!(server.requests().is_empty());
    assert_eq!(installed.state.version.as_deref(), Some("1.0.0"));
    let install_transition = &install_plan.plan.plan.packages[0];
    let installed_state = install_transition.after.as_ref().unwrap();
    assert_granted(
        &home,
        MANAGED_SCOPE_ID,
        &installed_state.release.package_sha256,
        &installed_state.permissions,
    )
    .await;

    let upgrade_lock = resolve_remote_package_lock(
        resolved.root(),
        resolved.dependencies(),
        "acme/worker",
        Some("2.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let upgrade_candidate = upgrade_lock.package("acme/worker").unwrap().catalog.clone();
    let upgrade_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:offline-upgrade:0001".to_owned(),
        assignment_generation: 11,
        capabilities_digest: capabilities_digest.clone(),
        scope: managed_scope.clone(),
        action: PluginOperationAction::Upgrade,
        package_id: package_id.clone(),
        candidate: Some(upgrade_candidate),
        package_lock: Some(upgrade_lock),
        selected_surfaces,
    };
    let upgrade_plan = host.plan(upgrade_request.clone()).await.unwrap();
    let upgrade_apply = apply_request(
        &upgrade_request,
        &upgrade_plan,
        "apply:offline-upgrade:0001",
    );
    drop(host);
    drop(resolved);
    drop(server);

    let restarted = CognitivePackageHostManager::new(
        managed_scope,
        "use:offline-apply-test",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let upgraded = restarted.apply(upgrade_apply.clone()).await.unwrap();
    assert!(!upgraded.replayed);
    assert_eq!(upgraded.state.version.as_deref(), Some("2.0.0"));
    let replayed = restarted.apply(upgrade_apply).await.unwrap();
    assert!(replayed.replayed);
    assert_eq!(
        replayed.operation_result_digest,
        upgraded.operation_result_digest
    );
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);

    let transition = &upgrade_plan.plan.plan.packages[0];
    let prior = transition.before.as_ref().unwrap();
    let candidate = transition.after.as_ref().unwrap();
    assert_revoked(&home, MANAGED_SCOPE_ID, &prior.release.package_sha256).await;
    assert_granted(
        &home,
        MANAGED_SCOPE_ID,
        &candidate.release.package_sha256,
        &candidate.permissions,
    )
    .await;
}

fn apply_request(
    plan_request: &PluginHostPlanRequest,
    plan: &a3s_use_core::PluginHostPlanResult,
    request_id: &str,
) -> PluginHostApplyRequest {
    PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: request_id.to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: plan_request.capabilities_digest.clone(),
        scope: plan_request.scope.clone(),
        package_id: plan_request.package_id.clone(),
        operation_id: plan.plan.plan.operation_id.clone(),
        plan_digest: plan.plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: plan.plan.plan.operation_id.clone(),
            plan_digest: plan.plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: plan.plan.plan.created_at_ms + 1,
        }),
    }
}
