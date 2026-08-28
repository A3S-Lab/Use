use super::*;

#[tokio::test]
async fn reviewed_host_plan_reproduces_exact_signed_lock_and_grant_in_a_clean_workspace() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        cognitive_tool_targets_version(
            temporary.path(),
            "acme/worker",
            "worker-reviewed",
            "1.0.0",
            &target,
        ),
        59,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());

    let source_home = temporary.path().join("source-home");
    let source_registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        source_home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let reviewed_scope = PlanScope {
        kind: PlanScopeKind::Workspace,
        id: MANAGED_SCOPE_ID.to_string(),
    };
    let source_extension_registry = ExtensionRegistry::new(
        ExtensionPaths::new(
            source_home.join("data"),
            source_home.join("state"),
            reviewed_scope.clone(),
        )
        .unwrap(),
    );
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let source_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        source_extension_registry.clone(),
        reviewed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let package_lock = resolve_remote_package_lock(
        &source_registry,
        &[],
        "acme/worker",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let expected_lock_digest = package_lock.descriptor_digest().unwrap();
    let reviewed = source_manager
        .prepare_install_remote(
            &source_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &expected_lock_digest,
        )
        .await
        .unwrap();
    let replayed_plan = source_manager
        .prepare_install_remote(
            &source_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &expected_lock_digest,
        )
        .await
        .unwrap();
    assert_eq!(replayed_plan, reviewed);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);
    assert!(source_manager
        .installed_package_lock("acme/worker")
        .await
        .unwrap()
        .is_none());
    assert!(source_extension_registry
        .get("acme/worker")
        .await
        .unwrap()
        .is_none());
    assert!(reviewed.package_lock.is_some());
    assert_eq!(reviewed.plan.scope, reviewed_scope);
    assert_eq!(reviewed.plan.workspace_impacts.len(), 1);
    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: reviewed.plan.operation_id.clone(),
        plan_digest: reviewed.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: reviewed.plan.created_at_ms + 1,
    };
    assert_eq!(
        reviewed
            .package_lock
            .as_ref()
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        expected_lock_digest
    );

    let target_home = temporary.path().join("target-home");
    let target_registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        target_home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let target_extension_registry = ExtensionRegistry::new(
        ExtensionPaths::new(
            target_home.join("data"),
            target_home.join("state"),
            reviewed_scope.clone(),
        )
        .unwrap(),
    );
    let target_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        target_extension_registry.clone(),
        reviewed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(
            ReviewedCognitivePackageAuthorizationProvider::new(
                reviewed.clone(),
                Some(confirmation.clone()),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let registry_lock = exclusive_lock(
        &extension_paths_for(&target_home, reviewed_scope.clone())
            .state_root()
            .join("extensions/.registry.lock"),
    );
    let interrupted = target_manager
        .install_remote(
            &target_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&expected_lock_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let mut drifted = reviewed.clone();
    let drifted_lock = drifted.package_lock.as_mut().unwrap();
    drifted_lock.packages[0]
        .catalog
        .provenance
        .timestamp_version += 1;
    let drifted_provenance = drifted_lock.packages[0].catalog.provenance.clone();
    let PluginPlanSource::Registry { provenance, .. } =
        drifted.plan.packages[0].source.as_mut().unwrap()
    else {
        panic!("reviewed signed package plan must retain Registry provenance");
    };
    *provenance = drifted_provenance;
    drifted.plan.package_lock_digest = Some(drifted_lock.descriptor_digest().unwrap());
    drifted.plan_digest = drifted.plan.descriptor_digest().unwrap();
    drifted.validate().unwrap();
    let drifted_confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: drifted.plan.operation_id.clone(),
        plan_digest: drifted.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: confirmation.confirmed_at_ms,
    };
    let drifted_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        target_extension_registry.clone(),
        reviewed_scope.clone(),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(
            ReviewedCognitivePackageAuthorizationProvider::new(drifted, Some(drifted_confirmation))
                .unwrap(),
        ),
    )
    .unwrap();
    let replay_error = drifted_manager
        .install_remote(
            &target_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&expected_lock_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(
        replay_error.code,
        "use.plugin.package_reviewed_plan_mismatch"
    );

    let replay_manager = CognitivePackageManager::with_plan_scope_lifecycle_and_authorization(
        target_extension_registry,
        reviewed_scope,
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(
            ReviewedCognitivePackageAuthorizationProvider::new(
                reviewed.clone(),
                Some(confirmation),
            )
            .unwrap(),
        ),
    )
    .unwrap();
    let target_result = replay_manager
        .install_remote(
            &target_registry,
            &[],
            "acme/worker",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            Some(&expected_lock_digest),
        )
        .await
        .unwrap();

    assert!(target_result.changed);
    assert_eq!(target_result.plan.as_ref(), Some(&reviewed));
    let installed = reviewed.plan.packages[0].after.as_ref().unwrap();
    assert_granted(
        &target_home,
        replay_manager.scope(),
        &installed.release.package_sha256,
        &installed.permissions,
    )
    .await;
}
