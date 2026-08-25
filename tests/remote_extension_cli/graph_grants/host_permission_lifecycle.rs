use super::*;

const PACKAGE_ID: &str = "acme/scope-kind-permission";
const ROUTE: &str = "scope-kind-permission";
const SCOPE_ID: &str = "shared:research";

#[tokio::test]
async fn permission_bearing_tool_lifecycle_replays_for_each_scope_kind() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        &temporary.path().join("v1"),
        PACKAGE_ID,
        ROUTE,
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        &temporary.path().join("v2"),
        PACKAGE_ID,
        ROUTE,
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 123, FUTURE);
    let server = TestServer::start(repository.routes.clone());

    for (scope_kind, label) in [
        (PlanScopeKind::User, "user"),
        (PlanScopeKind::Workspace, "workspace"),
    ] {
        let home = temporary.path().join(format!("{label}-permission-home"));
        let paths = ExtensionPaths::new(home.join("data"), home.join("state"));
        RegistrySourceStore::new(paths.clone())
            .add(RegistrySourceInput::new(
                "fixture",
                server.base_url(),
                &repository.root_sha256,
                None,
                VerifiedTargetCachePolicy::default(),
            ))
            .await
            .unwrap();

        let scope = managed_scope(scope_kind);
        let host = managed_host(&scope, paths.clone());
        let capabilities_digest = host
            .capabilities()
            .await
            .unwrap()
            .descriptor_digest()
            .unwrap();
        let package_id = PluginPackageId::parse(PACKAGE_ID).unwrap();
        let selected_surfaces = vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        }];

        let (install_request, install_plan, install_apply) = plan_operation(
            &host,
            &scope,
            &capabilities_digest,
            &package_id,
            PluginOperationAction::Install,
            "1.0.0",
            selected_surfaces.clone(),
            "install",
            None,
        )
        .await;
        assert_eq!(install_plan.plan.plan.scope.kind, scope_kind);
        let mut wrong_plan = install_request.clone();
        wrong_plan.scope = opposite_scope(&scope);
        assert_eq!(
            host.plan(wrong_plan).await.unwrap_err().code,
            "use.plugin.managed_scope_fence_mismatch"
        );
        let installed = host.apply(install_apply.clone()).await.unwrap();
        assert!(!installed.replayed);
        assert_eq!(installed.state.version.as_deref(), Some("1.0.0"));
        assert_eq!(installed.state.desired, PluginDesiredState::Enabled);
        assert_eq!(installed.state.observed, PluginObservedState::Ready);
        assert_grant(
            &home,
            &scope,
            install_plan.plan.plan.packages[0]
                .after
                .as_ref()
                .expect("install state")
                .release
                .package_sha256
                .as_str(),
            &install_plan.plan.plan.packages[0]
                .after
                .as_ref()
                .expect("install state")
                .permissions,
        )
        .await;
        assert_operation_completed(
            &host,
            &scope,
            &capabilities_digest,
            &package_id,
            &install_plan,
            "observe:install",
            &installed.operation_result_digest,
        )
        .await;

        let wrong_scope = opposite_scope(&scope);
        let mut wrong_apply = install_apply.clone();
        wrong_apply.scope = wrong_scope.clone();
        assert_eq!(
            host.apply(wrong_apply).await.unwrap_err().code,
            "use.plugin.managed_scope_fence_mismatch"
        );
        let wrong_observation = PluginHostOperationObservationRequest {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: "observe:install:wrong-scope".to_owned(),
            assignment_generation: 1,
            capabilities_digest: capabilities_digest.clone(),
            scope: wrong_scope,
            package_id: package_id.clone(),
            operation_id: install_plan.plan.plan.operation_id.clone(),
            plan_digest: install_plan.plan.plan_digest.clone(),
        };
        assert_eq!(
            host.observe_operation(wrong_observation.clone())
                .await
                .unwrap_err()
                .code,
            "use.plugin.managed_scope_fence_mismatch"
        );
        drop(host);
        let restarted = managed_host(&scope, paths.clone());
        let replayed_install = restarted.apply(install_apply).await.unwrap();
        assert!(replayed_install.replayed);
        assert_eq!(
            replayed_install.operation_result_digest,
            installed.operation_result_digest
        );
        assert_package_state(
            &restarted,
            &scope,
            &capabilities_digest,
            PluginDesiredState::Enabled,
            PluginObservedState::Ready,
            Some("1.0.0"),
            "observe:install:restart",
        )
        .await;

        let (upgrade_request, upgrade_plan, upgrade_apply) = plan_operation(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            PluginOperationAction::Upgrade,
            "2.0.0",
            selected_surfaces.clone(),
            "upgrade",
            Some(install_request.package_lock.clone().unwrap()),
        )
        .await;
        let upgraded = restarted.apply(upgrade_apply.clone()).await.unwrap();
        assert!(!upgraded.replayed);
        assert_eq!(upgraded.state.version.as_deref(), Some("2.0.0"));
        assert_eq!(upgraded.state.observed, PluginObservedState::Ready);
        let upgrade_transition = &upgrade_plan.plan.plan.packages[0];
        let prior = upgrade_transition
            .before
            .as_ref()
            .expect("upgrade prior state");
        let candidate = upgrade_transition.after.as_ref().expect("upgrade state");
        assert_revoked_local(&home, SCOPE_ID, &prior.release.package_sha256).await;
        assert_grant(
            &home,
            &scope,
            &candidate.release.package_sha256,
            &candidate.permissions,
        )
        .await;
        assert_operation_completed(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            &upgrade_plan,
            "observe:upgrade",
            &upgraded.operation_result_digest,
        )
        .await;

        drop(restarted);
        let restarted = managed_host(&scope, paths.clone());
        let replayed_upgrade = restarted.apply(upgrade_apply).await.unwrap();
        assert!(replayed_upgrade.replayed);
        assert_eq!(
            replayed_upgrade.operation_result_digest,
            upgraded.operation_result_digest
        );
        assert_package_state(
            &restarted,
            &scope,
            &capabilities_digest,
            PluginDesiredState::Enabled,
            PluginObservedState::Ready,
            Some("2.0.0"),
            "observe:upgrade:restart",
        )
        .await;

        let (_uninstall_request, uninstall_plan, uninstall_apply) = plan_operation(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            PluginOperationAction::Uninstall,
            "2.0.0",
            Vec::new(),
            "uninstall",
            Some(upgrade_request.package_lock.clone().unwrap()),
        )
        .await;
        let uninstalled = restarted.apply(uninstall_apply.clone()).await.unwrap();
        assert!(!uninstalled.replayed);
        assert_eq!(uninstalled.state.desired, PluginDesiredState::Absent);
        assert_eq!(uninstalled.state.observed, PluginObservedState::Removed);
        assert_revoked_local(&home, SCOPE_ID, &candidate.release.package_sha256).await;
        assert_operation_completed(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            &uninstall_plan,
            "observe:uninstall",
            &uninstalled.operation_result_digest,
        )
        .await;

        drop(restarted);
        let restarted = managed_host(&scope, paths);
        let replayed_uninstall = restarted.apply(uninstall_apply).await.unwrap();
        assert!(replayed_uninstall.replayed);
        assert_eq!(
            replayed_uninstall.operation_result_digest,
            uninstalled.operation_result_digest
        );
        assert_package_state(
            &restarted,
            &scope,
            &capabilities_digest,
            PluginDesiredState::Absent,
            PluginObservedState::Removed,
            None,
            "observe:uninstall:restart",
        )
        .await;
    }
}

fn managed_scope(kind: PlanScopeKind) -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:scope-kind-permission".to_owned(),
        scope_kind: kind,
        scope_id: SCOPE_ID.to_owned(),
        authority_id: "scope-kind:permission".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "e".repeat(64)),
    }
}

fn opposite_scope(scope: &PluginManagedScope) -> PluginManagedScope {
    let mut opposite = scope.clone();
    opposite.scope_kind = match scope.scope_kind {
        PlanScopeKind::User => PlanScopeKind::Workspace,
        PlanScopeKind::Workspace => PlanScopeKind::User,
    };
    opposite
}

fn managed_host(scope: &PluginManagedScope, paths: ExtensionPaths) -> CognitivePackageHostManager {
    CognitivePackageHostManager::new(
        scope.clone(),
        "use:scope-kind-permission",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
async fn plan_operation(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    package_id: &PluginPackageId,
    action: PluginOperationAction,
    version: &str,
    selected_surfaces: Vec<PluginSurfaceRef>,
    label: &str,
    prior_lock: Option<a3s_use_core::PluginPackageLock>,
) -> (
    PluginHostPlanRequest,
    a3s_use_core::PluginHostPlanResult,
    PluginHostApplyRequest,
) {
    let (candidate, package_lock) = if action == PluginOperationAction::Uninstall {
        (None, prior_lock)
    } else {
        let candidate = host
            .search_cognitive_packages(
                CognitiveRegistryAccess::Refreshed,
                Some("fixture"),
                &PluginCatalogSearch {
                    query: ROUTE.to_owned(),
                    kind: Some(PluginSurfaceKind::Tool),
                    channel: Some(PluginReleaseChannel::Stable),
                    publisher: Some("acme".to_owned()),
                    category: None,
                    availability: None,
                    cursor: None,
                    limit: 20,
                },
            )
            .await
            .unwrap()
            .plugins
            .into_iter()
            .find(|candidate| candidate.record.version == version)
            .unwrap_or_else(|| panic!("Registry search omitted {PACKAGE_ID} {version}"));
        let lock = host
            .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
            .await
            .unwrap();
        (Some(candidate), Some(lock))
    };
    let request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: format!("plan:scope-kind-permission:{label}"),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.to_owned(),
        scope: scope.clone(),
        action,
        package_id: package_id.clone(),
        candidate,
        package_lock,
        selected_surfaces,
    };
    let planned = host
        .plan(request.clone())
        .await
        .unwrap_or_else(|error| panic!("{action:?} planning failed: {error:?}"));
    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: format!("apply:scope-kind-permission:{label}"),
        assignment_generation: request.assignment_generation,
        capabilities_digest: request.capabilities_digest.clone(),
        scope: request.scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: planned.plan.plan.operation_id.clone(),
            plan_digest: planned.plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: planned.plan.plan.created_at_ms + 1,
        }),
    };
    (request, planned, apply)
}

async fn assert_grant(
    home: &std::path::Path,
    scope: &PluginManagedScope,
    package_digest: &str,
    ceiling: &a3s_use_core::PluginPermissionCeiling,
) {
    let record = WorkspaceGrantStore::new(home.join("state"))
        .observe(&scope.scope_id, PACKAGE_ID, package_digest)
        .await
        .unwrap()
        .unwrap();
    let StoredWorkspaceGrant::Granted(receipt) = record else {
        panic!("expected an active Grant receipt");
    };
    receipt.grant.validate_against(ceiling).unwrap();
    assert_eq!(receipt.grant.scope_id, scope.scope_id);
    assert_eq!(receipt.grant.package_id, PACKAGE_ID);
}

async fn assert_revoked_local(home: &std::path::Path, scope_id: &str, package_digest: &str) {
    let record = WorkspaceGrantStore::new(home.join("state"))
        .observe(scope_id, PACKAGE_ID, package_digest)
        .await
        .unwrap()
        .unwrap();
    let StoredWorkspaceGrant::Revoked(revocation) = record else {
        panic!("expected an exact Grant revocation");
    };
    assert_eq!(revocation.package_digest, package_digest);
    assert_eq!(revocation.package_id, PACKAGE_ID);
}

async fn assert_operation_completed(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    package_id: &PluginPackageId,
    planned: &a3s_use_core::PluginHostPlanResult,
    request_id: &str,
    result_digest: &str,
) {
    let observed = host
        .observe_operation(PluginHostOperationObservationRequest {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: request_id.to_owned(),
            assignment_generation: 1,
            capabilities_digest: capabilities_digest.to_owned(),
            scope: scope.clone(),
            package_id: package_id.clone(),
            operation_id: planned.plan.plan.operation_id.clone(),
            plan_digest: planned.plan.plan_digest.clone(),
        })
        .await
        .unwrap();
    assert_eq!(observed.status.phase, PluginHostOperationPhase::Completed);
    assert_eq!(
        observed.status.operation_result_digest.as_deref(),
        Some(result_digest)
    );
}

async fn assert_package_state(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    desired: PluginDesiredState,
    observed_state: PluginObservedState,
    version: Option<&str>,
    request_id: &str,
) {
    let result = host
        .observe(PluginHostObservationRequest {
            schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: request_id.to_owned(),
            assignment_generation: 1,
            capabilities_digest: capabilities_digest.to_owned(),
            scope: scope.clone(),
            package_id: PluginPackageId::parse(PACKAGE_ID).unwrap(),
        })
        .await
        .unwrap();
    let PluginHostObservationStatus::Available { state } = result.status else {
        panic!("permission lifecycle package observation was unavailable");
    };
    assert_eq!(state.desired, desired);
    assert_eq!(state.observed, observed_state);
    assert_eq!(state.version.as_deref(), version);
}
