use super::*;

#[tokio::test]
async fn host_manager_binds_same_textual_id_to_exact_scope_kind() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        vec![
            cognitive_skill_target(
                &temporary.path().join("workspace"),
                "acme/workspace-skill",
                "workspace-skill",
                Vec::new(),
                &target,
            ),
            cognitive_skill_target(
                &temporary.path().join("user"),
                "acme/user-skill",
                "user-skill",
                Vec::new(),
                &target,
            ),
        ],
        71,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("scope-kind-host-home");
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

    let workspace_scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:scope-kind".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "shared:research".to_owned(),
        authority_id: "scope-kind:user".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "6".repeat(64)),
    };
    let user_scope = PluginManagedScope {
        scope_kind: PlanScopeKind::User,
        ..workspace_scope.clone()
    };
    assert_ne!(
        workspace_scope.descriptor_digest().unwrap(),
        user_scope.descriptor_digest().unwrap()
    );

    let authorization = || {
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        })
    };
    let lifecycle = || Arc::new(StandaloneCognitivePackageLifecycleFactory::default());
    let workspace_host = CognitivePackageHostManager::new(
        workspace_scope.clone(),
        "use:scope-kind-test",
        ExtensionRegistry::new(paths.clone()),
        lifecycle(),
        authorization(),
    )
    .unwrap();
    let user_host = CognitivePackageHostManager::new(
        user_scope.clone(),
        "use:scope-kind-test",
        ExtensionRegistry::new(paths.clone()),
        lifecycle(),
        authorization(),
    )
    .unwrap();

    let workspace_candidate = workspace_host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "workspace-skill".to_owned(),
                kind: Some(PluginSurfaceKind::Skill),
                channel: Some(PluginReleaseChannel::Stable),
                publisher: None,
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
        .next()
        .unwrap();
    let workspace_lock = workspace_host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &workspace_candidate)
        .await
        .unwrap();
    let user_candidate = user_host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "user-skill".to_owned(),
                kind: Some(PluginSurfaceKind::Skill),
                channel: Some(PluginReleaseChannel::Stable),
                publisher: None,
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
        .next()
        .unwrap();
    let user_lock = user_host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &user_candidate)
        .await
        .unwrap();
    let capabilities = workspace_host.capabilities().await.unwrap();
    assert_eq!(capabilities, user_host.capabilities().await.unwrap());
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let selected_surfaces = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Skill,
        id: "main".to_owned(),
    }];
    let workspace_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:scope-kind:0001".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.clone(),
        scope: workspace_scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/workspace-skill").unwrap(),
        candidate: Some(workspace_candidate),
        package_lock: Some(workspace_lock),
        selected_surfaces: selected_surfaces.clone(),
    };
    let user_request = PluginHostPlanRequest {
        scope: user_scope.clone(),
        package_id: PluginPackageId::parse("acme/user-skill").unwrap(),
        candidate: Some(user_candidate),
        package_lock: Some(user_lock),
        selected_surfaces,
        ..workspace_request.clone()
    };

    let workspace_plan = workspace_host
        .plan(workspace_request.clone())
        .await
        .unwrap();
    let user_plan = user_host.plan(user_request.clone()).await.unwrap();
    assert_eq!(
        workspace_plan.plan.plan.scope.kind,
        PlanScopeKind::Workspace
    );
    assert_eq!(user_plan.plan.plan.scope.kind, PlanScopeKind::User);
    assert_eq!(
        workspace_plan.plan.plan.scope.id,
        user_plan.plan.plan.scope.id
    );
    assert_ne!(workspace_plan.plan.plan_digest, user_plan.plan.plan_digest);
    assert!(!workspace_plan.replayed);
    assert!(!user_plan.replayed);

    let error = workspace_host.plan(user_request.clone()).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");
    let error = user_host.plan(workspace_request.clone()).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    drop(workspace_host);
    drop(user_host);
    let restarted_workspace = CognitivePackageHostManager::new(
        workspace_scope,
        "use:scope-kind-test",
        ExtensionRegistry::new(paths.clone()),
        lifecycle(),
        authorization(),
    )
    .unwrap();
    let restarted_user = CognitivePackageHostManager::new(
        user_scope,
        "use:scope-kind-test",
        ExtensionRegistry::new(paths),
        lifecycle(),
        authorization(),
    )
    .unwrap();
    assert!(
        restarted_workspace
            .plan(workspace_request)
            .await
            .unwrap()
            .replayed
    );
    assert!(restarted_user.plan(user_request).await.unwrap().replayed);
}
