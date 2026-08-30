use super::*;

const LIFECYCLE_PACKAGE_ID: &str = "acme/scope-kind-lifecycle";
const LIFECYCLE_ROUTE: &str = "scope-kind-lifecycle";
const LIFECYCLE_SCOPE_ID: &str = "shared:research";

#[tokio::test]
async fn host_manager_replays_the_full_lifecycle_for_each_scope_kind() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        vec![
            cognitive_skill_target_version(
                &temporary.path().join("v1"),
                LIFECYCLE_PACKAGE_ID,
                LIFECYCLE_ROUTE,
                "1.0.0",
                Vec::new(),
                &target,
            ),
            cognitive_skill_target_version(
                &temporary.path().join("v2"),
                LIFECYCLE_PACKAGE_ID,
                LIFECYCLE_ROUTE,
                "2.0.0",
                Vec::new(),
                &target,
            ),
        ],
        72,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());

    for (scope_kind, label) in [
        (PlanScopeKind::User, "user"),
        (PlanScopeKind::Workspace, "workspace"),
    ] {
        let home = temporary.path().join(format!("{label}-lifecycle-home"));
        let scope = lifecycle_scope(scope_kind);
        let paths = managed_extension_paths(&home, &scope);
        RegistrySourceStore::new(use_paths(&home))
            .add(RegistrySourceInput::new(
                "fixture",
                server.base_url(),
                &repository.root_sha256,
                None,
                VerifiedTargetCachePolicy::default(),
            ))
            .await
            .unwrap();

        let host = lifecycle_host(&scope, paths.clone());
        let capabilities_digest = host
            .capabilities()
            .await
            .unwrap()
            .descriptor_digest()
            .unwrap();
        let package_id = PluginPackageId::parse(LIFECYCLE_PACKAGE_ID).unwrap();
        let selected_surfaces = vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "main".to_owned(),
        }];
        let context = HostLifecycleContext {
            host: &host,
            scope: &scope,
            capabilities_digest: &capabilities_digest,
            package_id: &package_id,
            search_query: LIFECYCLE_ROUTE,
            surface_kind: PluginSurfaceKind::Skill,
            registry_access: CognitiveRegistryAccess::Refreshed,
        };

        let (install_request, install_plan, install_apply) = host_lifecycle_release_operation(
            &context,
            PluginOperationAction::Install,
            "1.0.0",
            selected_surfaces.clone(),
            "install",
            None,
        )
        .await;
        assert!(!install_plan.replayed);
        assert_eq!(install_plan.plan.plan.scope.kind, scope_kind);
        assert_eq!(install_plan.plan.plan.scope.id, LIFECYCLE_SCOPE_ID);
        assert_scope_kind_substitution_rejected(
            &host,
            &install_request,
            &install_plan,
            &install_apply,
            opposite_scope(&scope),
        )
        .await;
        let installed = host.apply(install_apply.clone()).await.unwrap();
        assert!(!installed.replayed);
        assert_eq!(installed.state.version.as_deref(), Some("1.0.0"));
        assert_eq!(installed.state.desired, PluginDesiredState::Enabled);
        assert_eq!(installed.state.selected_surfaces, selected_surfaces);
        assert_completed_operation(
            &host,
            &scope,
            &capabilities_digest,
            &package_id,
            &install_plan,
            "observe:install",
            &installed.operation_result_digest,
        )
        .await;

        drop(host);
        let restarted = lifecycle_host(&scope, paths.clone());
        let restarted_context = HostLifecycleContext {
            host: &restarted,
            scope: &scope,
            capabilities_digest: &capabilities_digest,
            package_id: &package_id,
            search_query: LIFECYCLE_ROUTE,
            surface_kind: PluginSurfaceKind::Skill,
            registry_access: CognitiveRegistryAccess::Refreshed,
        };
        let replayed_install = restarted.apply(install_apply.clone()).await.unwrap();
        assert!(replayed_install.replayed);
        assert_eq!(
            replayed_install.operation_result_digest,
            installed.operation_result_digest
        );
        assert_package_state(
            &restarted_context,
            PluginDesiredState::Enabled,
            PluginObservedState::Ready,
            Some("1.0.0"),
            "observe:install:restart",
        )
        .await;
        assert_scope_observation_substitution_rejected(
            &restarted,
            &scope,
            &capabilities_digest,
            &package_id,
            opposite_scope(&scope),
            "observe:install:wrong-scope",
        )
        .await;

        let (upgrade_request, upgrade_plan, upgrade_apply) = host_lifecycle_release_operation(
            &restarted_context,
            PluginOperationAction::Upgrade,
            "2.0.0",
            selected_surfaces.clone(),
            "upgrade",
            Some(install_request.package_lock.clone().unwrap()),
        )
        .await;
        assert_scope_kind_substitution_rejected(
            &restarted,
            &upgrade_request,
            &upgrade_plan,
            &upgrade_apply,
            opposite_scope(&scope),
        )
        .await;
        let upgraded = restarted.apply(upgrade_apply.clone()).await.unwrap();
        assert!(!upgraded.replayed);
        assert_eq!(upgraded.state.version.as_deref(), Some("2.0.0"));
        assert_eq!(upgraded.state.desired, PluginDesiredState::Enabled);
        assert_completed_operation(
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
        let restarted = lifecycle_host(&scope, paths.clone());
        let restarted_context = HostLifecycleContext {
            host: &restarted,
            scope: &scope,
            capabilities_digest: &capabilities_digest,
            package_id: &package_id,
            search_query: LIFECYCLE_ROUTE,
            surface_kind: PluginSurfaceKind::Skill,
            registry_access: CognitiveRegistryAccess::Refreshed,
        };
        let replayed_upgrade = restarted.apply(upgrade_apply.clone()).await.unwrap();
        assert!(replayed_upgrade.replayed);
        assert_eq!(
            replayed_upgrade.operation_result_digest,
            upgraded.operation_result_digest
        );
        assert_package_state(
            &restarted_context,
            PluginDesiredState::Enabled,
            PluginObservedState::Ready,
            Some("2.0.0"),
            "observe:upgrade:restart",
        )
        .await;

        let (uninstall_request, uninstall_plan, uninstall_apply) =
            host_lifecycle_release_operation(
                &restarted_context,
                PluginOperationAction::Uninstall,
                "2.0.0",
                Vec::new(),
                "uninstall",
                Some(upgrade_request.package_lock.clone().unwrap()),
            )
            .await;
        assert!(uninstall_request.candidate.is_none());
        assert_scope_kind_substitution_rejected(
            &restarted,
            &uninstall_request,
            &uninstall_plan,
            &uninstall_apply,
            opposite_scope(&scope),
        )
        .await;
        let uninstalled = restarted.apply(uninstall_apply.clone()).await.unwrap();
        assert!(!uninstalled.replayed);
        assert_eq!(uninstalled.state.desired, PluginDesiredState::Absent);
        assert_eq!(uninstalled.state.observed, PluginObservedState::Removed);
        assert!(uninstalled.state.version.is_none());
        assert_completed_operation(
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
        let restarted = lifecycle_host(&scope, paths);
        let restarted_context = HostLifecycleContext {
            host: &restarted,
            scope: &scope,
            capabilities_digest: &capabilities_digest,
            package_id: &package_id,
            search_query: LIFECYCLE_ROUTE,
            surface_kind: PluginSurfaceKind::Skill,
            registry_access: CognitiveRegistryAccess::Refreshed,
        };
        let replayed_uninstall = restarted.apply(uninstall_apply).await.unwrap();
        assert!(replayed_uninstall.replayed);
        assert_eq!(
            replayed_uninstall.operation_result_digest,
            uninstalled.operation_result_digest
        );
        assert_package_state(
            &restarted_context,
            PluginDesiredState::Absent,
            PluginObservedState::Removed,
            None,
            "observe:uninstall:restart",
        )
        .await;
    }
}

fn lifecycle_scope(kind: PlanScopeKind) -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:scope-kind-lifecycle".to_owned(),
        scope_kind: kind,
        scope_id: LIFECYCLE_SCOPE_ID.to_owned(),
        authority_id: "scope-kind:lifecycle".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "d".repeat(64)),
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

fn lifecycle_host(
    scope: &PluginManagedScope,
    paths: ExtensionPaths,
) -> CognitivePackageHostManager {
    CognitivePackageHostManager::new(
        scope.clone(),
        "use:scope-kind-lifecycle",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap()
}

async fn assert_scope_kind_substitution_rejected(
    host: &CognitivePackageHostManager,
    request: &PluginHostPlanRequest,
    planned: &a3s_use_core::PluginHostPlanResult,
    apply: &PluginHostApplyRequest,
    wrong_scope: PluginManagedScope,
) {
    let operation_id = planned.plan.plan.operation_id.clone();
    let plan_digest = planned.plan.plan_digest.clone();
    let mut wrong_plan = request.clone();
    wrong_plan.scope = wrong_scope.clone();
    let error = host.plan(wrong_plan).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    let mut wrong_apply = apply.clone();
    wrong_apply.scope = wrong_scope.clone();
    let error = host.apply(wrong_apply).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    let error = host
        .observe_operation(PluginHostOperationObservationRequest {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: "observe:scope-kind:wrong-scope".to_owned(),
            assignment_generation: request.assignment_generation,
            capabilities_digest: request.capabilities_digest.clone(),
            scope: wrong_scope,
            package_id: request.package_id.clone(),
            operation_id,
            plan_digest,
        })
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");
    assert_eq!(planned.plan.plan.scope.kind, request.scope.scope_kind);
}

async fn assert_scope_observation_substitution_rejected(
    host: &CognitivePackageHostManager,
    scope: &PluginManagedScope,
    capabilities_digest: &str,
    package_id: &PluginPackageId,
    wrong_scope: PluginManagedScope,
    request_id: &str,
) {
    let mut request = PluginHostObservationRequest {
        schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: request_id.to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.to_owned(),
        scope: scope.clone(),
        package_id: package_id.clone(),
    };
    request.scope = wrong_scope;
    let error = host.observe(request).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");
}

async fn assert_completed_operation(
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
    context: &HostLifecycleContext<'_>,
    desired: PluginDesiredState,
    observed_state: PluginObservedState,
    version: Option<&str>,
    request_id: &str,
) {
    let result = context
        .host
        .observe(PluginHostObservationRequest {
            schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: request_id.to_owned(),
            assignment_generation: 1,
            capabilities_digest: context.capabilities_digest.to_owned(),
            scope: context.scope.clone(),
            package_id: context.package_id.clone(),
        })
        .await
        .unwrap();
    let PluginHostObservationStatus::Available { state } = result.status else {
        panic!("scope lifecycle package observation was unavailable");
    };
    assert_eq!(state.desired, desired);
    assert_eq!(state.observed, observed_state);
    assert_eq!(state.version.as_deref(), version);
}
