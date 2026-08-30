use super::*;

const MATRIX_PACKAGE_ID: &str = "acme/knowledge";
const MATRIX_SCOPE_ID: &str = "shared:two-scope-matrix";
const MATRIX_SURFACE_ID: &str = "domain-knowledge";
const VERSION_ONE_DECISION: &str = "Generation one keeps the amber two-scope lifecycle invariant.";
const VERSION_TWO_DECISION: &str = "Generation two keeps the cobalt two-scope lifecycle invariant.";

#[test]
fn same_package_two_scope_matrix_preserves_exact_authority_and_leased_invocation() {
    const TEST_THREAD_STACK_SIZE: usize = 16 * 1024 * 1024;

    std::thread::Builder::new()
        .name("two-scope-lifecycle-matrix".to_owned())
        .stack_size(TEST_THREAD_STACK_SIZE)
        .spawn(|| {
            tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap()
                .block_on(two_scope_matrix_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn two_scope_matrix_scenario() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        vec![
            cognitive_okf_target(
                &temporary.path().join("matrix-v1"),
                "1.0.0",
                VERSION_ONE_DECISION,
                &target,
            ),
            cognitive_okf_target(
                &temporary.path().join("matrix-v2"),
                "2.0.0",
                VERSION_TWO_DECISION,
                &target,
            ),
        ],
        73,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("two-scope-matrix-home");
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

    let user_scope = matrix_scope(PlanScopeKind::User);
    let workspace_scope = matrix_scope(PlanScopeKind::Workspace);
    let user_paths = managed_extension_paths(&home, &user_scope);
    let workspace_paths = managed_extension_paths(&home, &workspace_scope);
    assert_ne!(user_paths.state_root(), workspace_paths.state_root());
    assert_eq!(
        user_paths.artifact_store().root(),
        workspace_paths.artifact_store().root()
    );

    let user_host = matrix_host(&user_scope, user_paths.clone());
    let workspace_host = matrix_host(&workspace_scope, workspace_paths.clone());
    let user_capabilities_digest = user_host
        .capabilities()
        .await
        .unwrap()
        .descriptor_digest()
        .unwrap();
    let workspace_capabilities_digest = workspace_host
        .capabilities()
        .await
        .unwrap()
        .descriptor_digest()
        .unwrap();
    assert_eq!(
        user_capabilities_digest, workspace_capabilities_digest,
        "scope identity must bind requests, not change the supported protocol"
    );
    let package_id = PluginPackageId::parse(MATRIX_PACKAGE_ID).unwrap();
    let selected_surfaces = matrix_surfaces();

    let user_context = matrix_context(
        &user_host,
        &user_scope,
        &user_capabilities_digest,
        &package_id,
        CognitiveRegistryAccess::Refreshed,
    );
    let (user_install_request, _, user_install_apply) = host_lifecycle_release_operation(
        &user_context,
        PluginOperationAction::Install,
        "1.0.0",
        selected_surfaces.clone(),
        "matrix:user:install",
        None,
    )
    .await;
    let user_installed = user_host.apply(user_install_apply.clone()).await.unwrap();
    assert_eq!(user_installed.state.version.as_deref(), Some("1.0.0"));

    let workspace_context = matrix_context(
        &workspace_host,
        &workspace_scope,
        &workspace_capabilities_digest,
        &package_id,
        CognitiveRegistryAccess::Cached,
    );
    let (workspace_install_request, _, workspace_install_apply) = host_lifecycle_release_operation(
        &workspace_context,
        PluginOperationAction::Install,
        "1.0.0",
        selected_surfaces.clone(),
        "matrix:workspace:install",
        None,
    )
    .await;
    let workspace_installed = workspace_host
        .apply(workspace_install_apply.clone())
        .await
        .unwrap();
    assert_eq!(workspace_installed.state.version.as_deref(), Some("1.0.0"));

    drop(user_host);
    drop(workspace_host);
    let user_host = matrix_host(&user_scope, user_paths.clone());
    let workspace_host = matrix_host(&workspace_scope, workspace_paths.clone());
    assert!(
        user_host
            .apply(user_install_apply.clone())
            .await
            .unwrap()
            .replayed
    );
    assert!(
        workspace_host
            .apply(workspace_install_apply.clone())
            .await
            .unwrap()
            .replayed
    );

    let user_projection = CapabilityRegistry::new(ExtensionRegistry::new(user_paths.clone()));
    let workspace_projection =
        CapabilityRegistry::new(ExtensionRegistry::new(workspace_paths.clone()));
    let user_v1_snapshot = user_projection.snapshot().await.unwrap();
    let workspace_v1_snapshot = workspace_projection.snapshot().await.unwrap();
    assert_eq!(user_v1_snapshot.installation, user_scope.plan_scope());
    assert_eq!(
        workspace_v1_snapshot.installation,
        workspace_scope.plan_scope()
    );
    assert_eq!(user_v1_snapshot.installation_generation, Some(1));
    assert_eq!(workspace_v1_snapshot.installation_generation, Some(1));
    assert_ne!(
        user_v1_snapshot.installation_snapshot_digest,
        workspace_v1_snapshot.installation_snapshot_digest
    );
    assert_eq!(user_v1_snapshot.cursor().packages.len(), 1);
    assert_eq!(
        user_v1_snapshot.cursor().packages,
        workspace_v1_snapshot.cursor().packages,
        "identical releases may share immutable evidence without sharing installation authority"
    );

    let user_v1_snapshot_lease = user_projection
        .acquire_snapshot_lease(user_v1_snapshot.cursor())
        .await
        .unwrap()
        .expect("the User capability snapshot must be leasable");
    let workspace_v1_snapshot_lease = workspace_projection
        .acquire_snapshot_lease(workspace_v1_snapshot.cursor())
        .await
        .unwrap()
        .expect("the Workspace capability snapshot must be leasable");
    assert_eq!(user_v1_snapshot_lease.package_count(), 1);
    assert_eq!(workspace_v1_snapshot_lease.package_count(), 1);
    user_v1_snapshot_lease
        .extension_lease()
        .verify_integrity()
        .await
        .unwrap();
    workspace_v1_snapshot_lease
        .extension_lease()
        .verify_integrity()
        .await
        .unwrap();
    let user_package_root = user_v1_snapshot_lease
        .extension_lease()
        .packages()
        .next()
        .unwrap()
        .receipt
        .package_root
        .clone();
    let workspace_package_root = workspace_v1_snapshot_lease
        .extension_lease()
        .packages()
        .next()
        .unwrap()
        .receipt
        .package_root
        .clone();
    assert_eq!(user_package_root, workspace_package_root);

    let error = user_projection
        .acquire_snapshot_lease(workspace_v1_snapshot.cursor())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.capability.snapshot_scope_mismatch");
    let error = workspace_projection
        .acquire_snapshot_lease(user_v1_snapshot.cursor())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.capability.snapshot_scope_mismatch");

    let user_v1_call = user_host
        .acquire_cognitive_capability(&user_scope, MATRIX_PACKAGE_ID, MATRIX_SURFACE_ID)
        .await
        .unwrap()
        .expect("the User v1 generation must admit an invocation");
    let workspace_v1_call = workspace_host
        .acquire_cognitive_capability(&workspace_scope, MATRIX_PACKAGE_ID, MATRIX_SURFACE_ID)
        .await
        .unwrap()
        .expect("the Workspace v1 generation must admit an invocation");
    assert_eq!(user_v1_call.evidence().scope, user_scope.plan_scope());
    assert_eq!(
        workspace_v1_call.evidence().scope,
        workspace_scope.plan_scope()
    );
    assert_knowledge_invocation(&user_v1_call, "amber", "1.0.0").await;
    assert_knowledge_invocation(&workspace_v1_call, "amber", "1.0.0").await;
    let error = match user_host
        .acquire_cognitive_capability(&workspace_scope, MATRIX_PACKAGE_ID, MATRIX_SURFACE_ID)
        .await
    {
        Err(error) => error,
        Ok(_) => panic!("a Workspace fence must not invoke through the User manager"),
    };
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    drop(user_v1_call);
    drop(user_v1_snapshot_lease);
    let user_context = matrix_context(
        &user_host,
        &user_scope,
        &user_capabilities_digest,
        &package_id,
        CognitiveRegistryAccess::Cached,
    );
    let (user_upgrade_request, _, user_upgrade_apply) = host_lifecycle_release_operation(
        &user_context,
        PluginOperationAction::Upgrade,
        "2.0.0",
        selected_surfaces.clone(),
        "matrix:user:upgrade",
        user_install_request.package_lock,
    )
    .await;
    let user_upgraded = user_host.apply(user_upgrade_apply.clone()).await.unwrap();
    assert_eq!(user_upgraded.state.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        workspace_projection.snapshot().await.unwrap().cursor(),
        workspace_v1_snapshot.cursor(),
        "a User upgrade must not advance Workspace authority"
    );
    assert_knowledge_invocation(&workspace_v1_call, "amber", "1.0.0").await;

    let user_v2_snapshot = user_projection.snapshot().await.unwrap();
    assert_eq!(user_v2_snapshot.installation_generation, Some(2));
    assert_ne!(user_v2_snapshot.cursor(), user_v1_snapshot.cursor());
    let user_v2_snapshot_lease = user_projection
        .acquire_snapshot_lease(user_v2_snapshot.cursor())
        .await
        .unwrap()
        .expect("the User v2 capability snapshot must be leasable");
    let user_v2_call = user_host
        .acquire_cognitive_capability(&user_scope, MATRIX_PACKAGE_ID, MATRIX_SURFACE_ID)
        .await
        .unwrap()
        .expect("the User v2 generation must admit an invocation");
    assert_knowledge_invocation(&user_v2_call, "cobalt", "2.0.0").await;

    drop(workspace_v1_call);
    drop(workspace_v1_snapshot_lease);
    let workspace_context = matrix_context(
        &workspace_host,
        &workspace_scope,
        &workspace_capabilities_digest,
        &package_id,
        CognitiveRegistryAccess::Cached,
    );
    let (workspace_upgrade_request, _, workspace_upgrade_apply) = host_lifecycle_release_operation(
        &workspace_context,
        PluginOperationAction::Upgrade,
        "2.0.0",
        selected_surfaces,
        "matrix:workspace:upgrade",
        workspace_install_request.package_lock,
    )
    .await;
    let workspace_upgraded = workspace_host
        .apply(workspace_upgrade_apply.clone())
        .await
        .unwrap();
    assert_eq!(workspace_upgraded.state.version.as_deref(), Some("2.0.0"));
    assert_eq!(
        user_projection.snapshot().await.unwrap().cursor(),
        user_v2_snapshot.cursor(),
        "a Workspace upgrade must not advance User authority"
    );
    assert_knowledge_invocation(&user_v2_call, "cobalt", "2.0.0").await;

    let workspace_v2_snapshot = workspace_projection.snapshot().await.unwrap();
    assert_eq!(workspace_v2_snapshot.installation_generation, Some(2));
    assert_ne!(
        workspace_v2_snapshot.cursor(),
        workspace_v1_snapshot.cursor()
    );
    let workspace_v2_snapshot_lease = workspace_projection
        .acquire_snapshot_lease(workspace_v2_snapshot.cursor())
        .await
        .unwrap()
        .expect("the Workspace v2 capability snapshot must be leasable");
    let workspace_v2_call = workspace_host
        .acquire_cognitive_capability(&workspace_scope, MATRIX_PACKAGE_ID, MATRIX_SURFACE_ID)
        .await
        .unwrap()
        .expect("the Workspace v2 generation must admit an invocation");
    assert_knowledge_invocation(&workspace_v2_call, "cobalt", "2.0.0").await;

    drop(user_v2_call);
    drop(user_v2_snapshot_lease);
    let user_context = matrix_context(
        &user_host,
        &user_scope,
        &user_capabilities_digest,
        &package_id,
        CognitiveRegistryAccess::Cached,
    );
    let (_, _, user_uninstall_apply) = host_lifecycle_release_operation(
        &user_context,
        PluginOperationAction::Uninstall,
        "2.0.0",
        Vec::new(),
        "matrix:user:uninstall",
        user_upgrade_request.package_lock,
    )
    .await;
    let user_uninstalled = user_host.apply(user_uninstall_apply.clone()).await.unwrap();
    assert_eq!(user_uninstalled.state.desired, PluginDesiredState::Absent);
    let user_absent_snapshot = user_projection.snapshot().await.unwrap();
    assert_eq!(user_absent_snapshot.installation_generation, Some(3));
    assert!(user_absent_snapshot.cursor().packages.is_empty());
    assert_eq!(
        workspace_projection.snapshot().await.unwrap().cursor(),
        workspace_v2_snapshot.cursor(),
        "a User uninstall must not advance Workspace authority"
    );
    assert_knowledge_invocation(&workspace_v2_call, "cobalt", "2.0.0").await;
    assert!(user_host
        .acquire_cognitive_capability(&user_scope, MATRIX_PACKAGE_ID, MATRIX_SURFACE_ID)
        .await
        .unwrap()
        .is_none());

    drop(workspace_v2_call);
    drop(workspace_v2_snapshot_lease);
    let workspace_context = matrix_context(
        &workspace_host,
        &workspace_scope,
        &workspace_capabilities_digest,
        &package_id,
        CognitiveRegistryAccess::Cached,
    );
    let (_, _, workspace_uninstall_apply) = host_lifecycle_release_operation(
        &workspace_context,
        PluginOperationAction::Uninstall,
        "2.0.0",
        Vec::new(),
        "matrix:workspace:uninstall",
        workspace_upgrade_request.package_lock,
    )
    .await;
    let workspace_uninstalled = workspace_host
        .apply(workspace_uninstall_apply.clone())
        .await
        .unwrap();
    assert_eq!(
        workspace_uninstalled.state.desired,
        PluginDesiredState::Absent
    );
    let workspace_absent_snapshot = workspace_projection.snapshot().await.unwrap();
    assert_eq!(workspace_absent_snapshot.installation_generation, Some(3));
    assert!(workspace_absent_snapshot.cursor().packages.is_empty());

    drop(user_host);
    drop(workspace_host);
    let restarted_user = matrix_host(&user_scope, user_paths);
    let restarted_workspace = matrix_host(&workspace_scope, workspace_paths);
    assert!(
        restarted_user
            .apply(user_uninstall_apply)
            .await
            .unwrap()
            .replayed
    );
    assert!(
        restarted_workspace
            .apply(workspace_uninstall_apply)
            .await
            .unwrap()
            .replayed
    );
}

fn matrix_scope(kind: PlanScopeKind) -> PluginManagedScope {
    PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:two-scope-matrix".to_owned(),
        scope_kind: kind,
        scope_id: MATRIX_SCOPE_ID.to_owned(),
        authority_id: "authority:two-scope-matrix".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "e".repeat(64)),
    }
}

fn matrix_host(scope: &PluginManagedScope, paths: ExtensionPaths) -> CognitivePackageHostManager {
    CognitivePackageHostManager::new(
        scope.clone(),
        "use:two-scope-matrix",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap()
}

fn matrix_context<'a>(
    host: &'a CognitivePackageHostManager,
    scope: &'a PluginManagedScope,
    capabilities_digest: &'a str,
    package_id: &'a PluginPackageId,
    registry_access: CognitiveRegistryAccess,
) -> HostLifecycleContext<'a> {
    HostLifecycleContext {
        host,
        scope,
        capabilities_digest,
        package_id,
        search_query: "knowledge",
        surface_kind: PluginSurfaceKind::Okf,
        registry_access,
    }
}

fn matrix_surfaces() -> Vec<PluginSurfaceRef> {
    vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Okf,
        id: MATRIX_SURFACE_ID.to_owned(),
    }]
}

async fn assert_knowledge_invocation(
    lease: &a3s_use::cognitive_package::CognitiveCapabilityLease,
    query: &str,
    version: &str,
) {
    assert_eq!(lease.evidence().package_version, version);
    let result = lease.knowledge().search(query, 4).await.unwrap();
    assert!(
        !result.hits.is_empty(),
        "the exact {version} leased generation must answer the query"
    );
}
