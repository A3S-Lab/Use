use super::*;

#[tokio::test]
async fn host_operation_observation_and_pre_admission_cancellation_are_exact() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        vec![cognitive_okf_target(
            temporary.path(),
            "1.0.0",
            "Cancellation remains before exact package admission.",
            &target,
        )],
        68,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("cancel-host-home");
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
    let scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:workbaby".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "workspace:cancel".to_owned(),
        authority_id: "workbaby:user".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "9".repeat(64)),
    };
    let host = CognitivePackageHostManager::new(
        scope.clone(),
        "use:cancel-test",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let candidate = host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "knowledge".to_owned(),
                kind: Some(PluginSurfaceKind::Okf),
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
    let lock = host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
        .await
        .unwrap();
    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:cancel:0001".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
        candidate: Some(candidate),
        package_lock: Some(lock),
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
        }],
    };
    let planned = host.plan(plan_request.clone()).await.unwrap();
    let observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:cancel:0001".to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        package_id: plan_request.package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
    };
    let observed = host.observe_operation(observation.clone()).await.unwrap();
    assert_eq!(
        observed.status.phase,
        PluginHostOperationPhase::AwaitingConfirmation
    );
    assert_eq!(
        observed.status.cancellability,
        PluginHostOperationCancellability::Cancellable
    );

    let cancellation = PluginHostCancelRequest {
        schema: PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned(),
        request_id: "cancel:cancel:0001".to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest,
        scope: scope.clone(),
        package_id: plan_request.package_id,
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        requested_by: PlanActor::User,
    };
    let cancelled = host.cancel(cancellation.clone()).await.unwrap();
    assert_eq!(cancelled.status, PluginHostCancellationStatus::Cancelled);
    let scope_digest = scope.descriptor_digest().unwrap();
    let cancellation_path = home
        .join("state/plugin-host-manager")
        .join(scope_digest.strip_prefix("sha256:").unwrap())
        .join("cancellations")
        .join(format!(
            "{:x}.json",
            Sha256::digest(planned.plan.plan.operation_id.as_bytes())
        ));
    std::fs::remove_file(&cancellation_path).unwrap();
    drop(host);
    let paths = ExtensionPaths::new(home.join("data"), home.join("state"));
    let restarted = CognitivePackageHostManager::new(
        scope.clone(),
        "use:cancel-test",
        ExtensionRegistry::new(paths),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let replayed = restarted.cancel(cancellation).await.unwrap();
    assert_eq!(
        replayed.status,
        PluginHostCancellationStatus::AlreadyCancelled
    );
    let observed = restarted.observe_operation(observation).await.unwrap();
    assert_eq!(observed.status.phase, PluginHostOperationPhase::Cancelled);
    assert_eq!(
        observed.status.cancellability,
        PluginHostOperationCancellability::NotApplicable
    );
    let requests_before_history = server.requests().len();
    let history = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            "acme/knowledge",
            "--history",
            "--scope-kind",
            "workspace",
            "--scope-id",
            "workspace:cancel",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(history.status.success(), "{history:?}");
    assert_eq!(server.requests().len(), requests_before_history);
    let history = json(&history);
    let history = &history["data"]["diagnostic"];
    assert_eq!(history["retainedOperationCount"], 1);
    assert_eq!(history["operations"][0]["outcome"], "cancelled");
    assert_eq!(
        history["operations"][0]["diagnostic"]["operation"]["phase"],
        "cancelled"
    );

    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:cancel:0001".to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: planned.capabilities_digest,
        scope: planned.scope,
        package_id: planned.package_id,
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: planned.plan.plan.operation_id,
            plan_digest: planned.plan.plan_digest,
            confirmed_by: PlanActor::User,
            confirmed_at_ms: planned.plan.plan.created_at_ms + 1,
        }),
    };
    let error = restarted.apply(apply).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_operation_cancelled");
}

#[tokio::test]
async fn host_cancel_is_too_late_after_exact_durable_admission_and_watch_times_out() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        vec![cognitive_okf_target(
            temporary.path(),
            "1.0.0",
            "Admission makes cancellation evidence exact.",
            &target,
        )],
        69,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("too-late-host-home");
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
    let scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_owned(),
        host_id: "host:workbaby".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "workspace:too-late".to_owned(),
        authority_id: "workbaby:user".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "8".repeat(64)),
    };
    let host = CognitivePackageHostManager::new(
        scope.clone(),
        "use:too-late-test",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: Arc::new(AtomicUsize::new(0)),
        }),
    )
    .unwrap();
    let candidate = host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "knowledge".to_owned(),
                kind: Some(PluginSurfaceKind::Okf),
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
    let lock = host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
        .await
        .unwrap();
    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:too-late:0001".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/knowledge").unwrap(),
        candidate: Some(candidate),
        package_lock: Some(lock),
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Okf,
            id: "domain-knowledge".to_owned(),
        }],
    };
    let planned = host.plan(request.clone()).await.unwrap();
    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:too-late:0001".to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
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
    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    let interrupted = host.apply(apply).await.unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");

    let cancellation = PluginHostCancelRequest {
        schema: PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned(),
        request_id: "cancel:too-late:0001".to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        package_id: request.package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        requested_by: PlanActor::User,
    };
    let cancelled = host.cancel(cancellation).await.unwrap();
    assert_eq!(cancelled.status, PluginHostCancellationStatus::TooLate);
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:too-late:0001".to_owned(),
        assignment_generation: request.assignment_generation,
        capabilities_digest,
        scope,
        package_id: request.package_id,
        operation_id: planned.plan.plan.operation_id,
        plan_digest: planned.plan.plan_digest,
    };
    let first = host.observe_operation(observation.clone()).await.unwrap();
    let watched = host
        .watch_operation(a3s_use_core::PluginHostOperationWatchRequest {
            schema: a3s_use_core::PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA.to_owned(),
            observation,
            after_revision: Some(first.revision.clone()),
            timeout_ms: 0,
        })
        .await
        .unwrap();
    assert!(!watched.changed);
    assert!(watched.timed_out);
    assert_eq!(watched.revision, first.revision);
}

#[tokio::test]
async fn production_host_manager_persists_plan_apply_replay_and_exact_fence() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let repository = TestRepository::with_targets(
        cognitive_tool_targets_version(
            temporary.path(),
            "acme/worker",
            "worker-host",
            "1.0.0",
            &target,
        ),
        61,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("host-home");
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
    let lock = resolve_remote_package_lock(
        resolved.root(),
        resolved.dependencies(),
        "acme/worker",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let candidate = lock.package("acme/worker").unwrap().catalog.clone();
    let managed_scope = PluginManagedScope {
        schema: PLUGIN_MANAGED_SCOPE_SCHEMA_V2.to_string(),
        host_id: "host:node-01".to_string(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: MANAGED_SCOPE_ID.to_string(),
        authority_id: "cloud:control-plane".to_string(),
        fence_generation: 7,
        fence_digest: format!("sha256:{}", "f".repeat(64)),
    };
    let authorization_count = Arc::new(AtomicUsize::new(0));
    let host = CognitivePackageHostManager::new(
        managed_scope.clone(),
        "use:test-build",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let selected_surfaces = vec![PluginSurfaceRef {
        kind: PluginSurfaceKind::Tool,
        id: "convert".to_string(),
    }];
    let plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_string(),
        request_id: "plan:worker:0001".to_string(),
        assignment_generation: 3,
        capabilities_digest: capabilities_digest.clone(),
        scope: managed_scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/worker".to_string()).unwrap(),
        candidate: Some(candidate),
        package_lock: Some(lock),
        selected_surfaces: selected_surfaces.clone(),
    };

    let planned = host.plan(plan_request.clone()).await.unwrap();
    assert!(!planned.replayed);
    assert_eq!(
        planned.plan.plan.packages[0]
            .after
            .as_ref()
            .unwrap()
            .release
            .surfaces
            .iter()
            .map(a3s_use_core::CatalogSurface::reference)
            .collect::<Vec<_>>(),
        selected_surfaces
    );
    let replayed_plan = host.plan(plan_request.clone()).await.unwrap();
    assert!(replayed_plan.replayed);
    assert_eq!(replayed_plan.plan, planned.plan);
    let mut conflicting_plan_request = plan_request.clone();
    conflicting_plan_request.assignment_generation += 1;
    let error = host.plan(conflicting_plan_request).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_store_conflict");

    let confirmation = PluginOperationConfirmation {
        schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmed_by: PlanActor::User,
        confirmed_at_ms: planned.plan.plan.created_at_ms + 1,
    };
    let apply_request = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_string(),
        request_id: "apply:worker:0001".to_string(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: managed_scope.clone(),
        package_id: plan_request.package_id.clone(),
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
        confirmation: Some(confirmation),
    };
    let mut unconfirmed_apply = apply_request.clone();
    unconfirmed_apply.request_id = "apply:worker:unconfirmed".to_string();
    unconfirmed_apply.confirmation = None;
    let error = host.apply(unconfirmed_apply).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.plan_confirmation_mismatch");
    server.clear_requests();
    let applied = host.apply(apply_request.clone()).await.unwrap();
    assert!(!applied.replayed);
    assert!(
        server.requests().is_empty(),
        "reviewed apply must consume only the exact artifacts cached during planning"
    );
    assert_eq!(applied.state.selected_surfaces, selected_surfaces);
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);

    let restarted = CognitivePackageHostManager::new(
        managed_scope.clone(),
        "use:test-build",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let replayed_apply = restarted.apply(apply_request.clone()).await.unwrap();
    assert!(replayed_apply.replayed);
    assert_eq!(
        replayed_apply.operation_result_digest,
        applied.operation_result_digest
    );

    let observe_request = PluginHostObservationRequest {
        schema: PLUGIN_HOST_OBSERVATION_REQUEST_SCHEMA.to_string(),
        request_id: "observe:worker:0001".to_string(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest,
        scope: managed_scope,
        package_id: plan_request.package_id,
    };
    let observed = restarted.observe(observe_request.clone()).await.unwrap();
    let PluginHostObservationStatus::Available { state } = observed.status else {
        panic!("the installed Host package must be observable");
    };
    assert_eq!(state.selected_surfaces, selected_surfaces);
    assert_eq!(state.desired, PluginDesiredState::Enabled);

    let cancelled_disable_request = PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:worker:disable:cancelled:0001".to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: observe_request.capabilities_digest.clone(),
        scope: observe_request.scope.clone(),
        package_id: observe_request.package_id.clone(),
        expected_package_generation: state.package_generation.unwrap(),
        enabled: false,
    };
    let cancelled_disable_plan = restarted
        .plan_enablement(cancelled_disable_request.clone())
        .await
        .unwrap();
    let cancelled_disable_envelope = cancelled_disable_plan.plan.as_ref().unwrap();
    let cancellation = PluginHostCancelRequest {
        schema: PLUGIN_HOST_CANCEL_REQUEST_SCHEMA.to_owned(),
        request_id: "cancel:worker:disable:0001".to_owned(),
        assignment_generation: cancelled_disable_request.assignment_generation,
        capabilities_digest: cancelled_disable_request.capabilities_digest.clone(),
        scope: cancelled_disable_request.scope.clone(),
        package_id: cancelled_disable_request.package_id.clone(),
        operation_id: cancelled_disable_envelope.plan.operation_id.clone(),
        plan_digest: cancelled_disable_envelope.plan_digest.clone(),
        requested_by: PlanActor::User,
    };
    let cancelled = restarted.cancel(cancellation.clone()).await.unwrap();
    assert_eq!(cancelled.status, PluginHostCancellationStatus::Cancelled);
    let replayed_cancellation = restarted.cancel(cancellation).await.unwrap();
    assert_eq!(
        replayed_cancellation.status,
        PluginHostCancellationStatus::AlreadyCancelled
    );
    let cancelled_observation = restarted
        .observe_operation(PluginHostOperationObservationRequest {
            schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
            request_id: "observe:worker:disable:cancelled:0001".to_owned(),
            assignment_generation: cancelled_disable_request.assignment_generation,
            capabilities_digest: cancelled_disable_request.capabilities_digest.clone(),
            scope: cancelled_disable_request.scope.clone(),
            package_id: cancelled_disable_request.package_id.clone(),
            operation_id: cancelled_disable_envelope.plan.operation_id.clone(),
            plan_digest: cancelled_disable_envelope.plan_digest.clone(),
        })
        .await
        .unwrap();
    assert_eq!(
        cancelled_observation.status.phase,
        PluginHostOperationPhase::Cancelled
    );
    let requests_before_diagnostic = server.requests().len();
    let authorizations_before_diagnostic = authorization_count.load(Ordering::SeqCst);
    let cancelled_diagnostic = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            "acme/worker",
            "--scope-kind",
            "workspace",
            "--scope-id",
            MANAGED_SCOPE_ID,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        cancelled_diagnostic.status.success(),
        "{cancelled_diagnostic:?}"
    );
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    assert_eq!(
        authorization_count.load(Ordering::SeqCst),
        authorizations_before_diagnostic
    );
    let cancelled_diagnostic = json(&cancelled_diagnostic);
    let cancelled_diagnostic = &cancelled_diagnostic["data"]["diagnostic"];
    assert_eq!(
        cancelled_diagnostic["schema"],
        "a3s.use.plugin-operation-diagnostic.v1"
    );
    assert_eq!(cancelled_diagnostic["scope"]["kind"], "workspace");
    assert_eq!(cancelled_diagnostic["scope"]["id"], MANAGED_SCOPE_ID);
    assert_eq!(cancelled_diagnostic["packageId"], "acme/worker");
    assert_eq!(
        cancelled_diagnostic["operation"]["operationId"],
        cancelled_disable_envelope.plan.operation_id
    );
    assert_eq!(
        cancelled_diagnostic["operation"]["planDigest"],
        cancelled_disable_envelope.plan_digest
    );
    assert_eq!(cancelled_diagnostic["operation"]["action"], "disable");
    assert_eq!(cancelled_diagnostic["operation"]["phase"], "cancelled");
    assert_eq!(
        cancelled_diagnostic["operation"]["confirmation"],
        "cancelled"
    );
    assert_eq!(cancelled_diagnostic["operation"]["lifecycleUnitCount"], 1);
    assert_eq!(
        cancelled_diagnostic["operation"]["observedLifecycleUnitCount"],
        0
    );
    assert_eq!(
        cancelled_diagnostic["operation"]["lifecycle"],
        serde_json::json!([])
    );
    assert_eq!(
        cancelled_diagnostic["operation"]["grant"]["status"],
        "cancelled"
    );
    assert_eq!(
        cancelled_diagnostic["operation"]["download"],
        "not-required"
    );
    assert_eq!(
        cancelled_diagnostic["operation"]["planning"],
        "not-required"
    );
    assert_eq!(
        cancelled_diagnostic["operation"]["recovery"],
        "observe-cancellation"
    );
    assert_eq!(
        cancelled_diagnostic["registry"]["operationCutover"]["status"],
        "not-observed"
    );
    let encoded_cancelled = serde_json::to_string(cancelled_diagnostic).unwrap();
    assert!(!encoded_cancelled.contains(home.to_str().unwrap()));
    assert!(!encoded_cancelled.contains(server.base_url()));
    assert!(!encoded_cancelled.contains("host:node-01"));
    assert!(!encoded_cancelled.contains("cloud:control-plane"));
    assert!(!encoded_cancelled.contains(&format!("sha256:{}", "f".repeat(64))));
    assert!(!encoded_cancelled.contains(&observe_request.capabilities_digest));
    assert!(!encoded_cancelled.contains("plan:worker:disable:cancelled:0001"));
    assert!(!encoded_cancelled.contains("cancel:worker:disable:0001"));
    let cancelled_apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:worker:disable:cancelled:0001".to_owned(),
        assignment_generation: cancelled_disable_request.assignment_generation,
        capabilities_digest: cancelled_disable_request.capabilities_digest,
        scope: cancelled_disable_request.scope,
        package_id: cancelled_disable_request.package_id,
        operation_id: cancelled_disable_envelope.plan.operation_id.clone(),
        plan_digest: cancelled_disable_envelope.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: cancelled_disable_envelope.plan.operation_id.clone(),
            plan_digest: cancelled_disable_envelope.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: cancelled_disable_envelope.plan.created_at_ms + 1,
        }),
    };
    let error = restarted.apply(cancelled_apply).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_operation_cancelled");

    let disable_request = PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_string(),
        request_id: "plan:worker:disable:0001".to_string(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: observe_request.capabilities_digest.clone(),
        scope: observe_request.scope.clone(),
        package_id: observe_request.package_id.clone(),
        expected_package_generation: state.package_generation.unwrap(),
        enabled: false,
    };
    let planned_disable = restarted
        .plan_enablement(disable_request.clone())
        .await
        .unwrap();
    assert_eq!(
        planned_disable.status,
        PluginHostEnablementPlanStatus::Planned
    );
    assert_eq!(planned_disable.state, state);
    assert!(!planned_disable.replayed);
    let replayed_disable_plan = restarted
        .plan_enablement(disable_request.clone())
        .await
        .unwrap();
    assert!(replayed_disable_plan.replayed);
    assert_eq!(replayed_disable_plan.plan, planned_disable.plan);

    let disable_plan = planned_disable.plan.as_ref().unwrap();
    let requests_before_diagnostic = server.requests().len();
    let authorizations_before_diagnostic = authorization_count.load(Ordering::SeqCst);
    let planned_diagnostic = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            "acme/worker",
            "--scope-kind",
            "workspace",
            "--scope-id",
            MANAGED_SCOPE_ID,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        planned_diagnostic.status.success(),
        "{planned_diagnostic:?}"
    );
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    assert_eq!(
        authorization_count.load(Ordering::SeqCst),
        authorizations_before_diagnostic
    );
    let planned_diagnostic = json(&planned_diagnostic);
    let planned_diagnostic = &planned_diagnostic["data"]["diagnostic"];
    assert_eq!(
        planned_diagnostic["schema"],
        "a3s.use.plugin-operation-diagnostic.v1"
    );
    assert_eq!(
        planned_diagnostic["operation"]["operationId"],
        disable_plan.plan.operation_id
    );
    assert_eq!(
        planned_diagnostic["operation"]["planDigest"],
        disable_plan.plan_digest
    );
    assert_eq!(planned_diagnostic["operation"]["action"], "disable");
    assert_eq!(planned_diagnostic["operation"]["phase"], "planned");
    assert_eq!(
        planned_diagnostic["operation"]["confirmation"],
        "awaiting-confirmation"
    );
    assert_eq!(planned_diagnostic["operation"]["lifecycleUnitCount"], 1);
    assert_eq!(
        planned_diagnostic["operation"]["observedLifecycleUnitCount"],
        0
    );
    assert_eq!(
        planned_diagnostic["operation"]["lifecycle"],
        serde_json::json!([])
    );
    let grant_required =
        disable_plan.plan.workspace_impacts.iter().any(|impact| {
            impact.grant_before_digest.is_some() || impact.grant_after_digest.is_some()
        });
    assert_eq!(
        planned_diagnostic["operation"]["grant"]["required"],
        grant_required
    );
    assert_eq!(
        planned_diagnostic["operation"]["grant"]["status"],
        if grant_required {
            "awaiting-admission"
        } else {
            "not-required"
        }
    );
    assert_eq!(planned_diagnostic["operation"]["download"], "not-required");
    assert_eq!(planned_diagnostic["operation"]["planning"], "not-required");
    assert_eq!(
        planned_diagnostic["operation"]["recovery"],
        "review-and-apply-exact-plan"
    );
    assert_eq!(
        planned_diagnostic["registry"]["operationCutover"]["status"],
        "not-observed"
    );
    assert_eq!(
        planned_diagnostic["operation"]["sources"][0]["kind"],
        "registry"
    );
    assert_eq!(
        planned_diagnostic["operation"]["sources"][0]["registryName"],
        "fixture"
    );
    let disable_apply_request = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_string(),
        request_id: "apply:worker:disable:0001".to_string(),
        assignment_generation: disable_request.assignment_generation,
        capabilities_digest: disable_request.capabilities_digest.clone(),
        scope: disable_request.scope.clone(),
        package_id: disable_request.package_id.clone(),
        operation_id: disable_plan.plan.operation_id.clone(),
        plan_digest: disable_plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: disable_plan.plan.operation_id.clone(),
            plan_digest: disable_plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: disable_plan.plan.created_at_ms + 1,
        }),
    };
    let disable_observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:worker:disable:0001".to_owned(),
        assignment_generation: disable_request.assignment_generation,
        capabilities_digest: disable_request.capabilities_digest.clone(),
        scope: disable_request.scope.clone(),
        package_id: disable_request.package_id.clone(),
        operation_id: disable_plan.plan.operation_id.clone(),
        plan_digest: disable_plan.plan_digest.clone(),
    };
    let disabled = apply_enablement_through_observed_windows(
        &restarted,
        &home,
        disable_apply_request.clone(),
        disable_observation,
    )
    .await;
    assert!(!disabled.replayed);
    assert_eq!(
        disabled.state.desired,
        PluginDesiredState::InstalledDisabled
    );
    assert!(
        disabled.state.package_generation.unwrap() > disable_request.expected_package_generation
    );

    let recovered = CognitivePackageHostManager::new(
        observe_request.scope.clone(),
        "use:test-build",
        ExtensionRegistry::new(paths.clone()),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ConfirmAllPlans {
            authorization_count: authorization_count.clone(),
        }),
    )
    .unwrap();
    let replayed_disable = recovered.apply(disable_apply_request).await.unwrap();
    assert!(replayed_disable.replayed);
    assert_eq!(
        replayed_disable.operation_result_digest,
        disabled.operation_result_digest
    );
    let requests_before_completed_diagnostic = server.requests().len();
    let completed_diagnostic = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            "acme/worker",
            "--scope-kind",
            "workspace",
            "--scope-id",
            MANAGED_SCOPE_ID,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        !completed_diagnostic.status.success(),
        "{completed_diagnostic:?}"
    );
    assert_eq!(
        server.requests().len(),
        requests_before_completed_diagnostic
    );
    assert_eq!(
        json(&completed_diagnostic)["error"]["code"],
        "use.plugin.operation_diagnostic_not_found"
    );

    let no_change = recovered
        .plan_enablement(PluginHostEnablementPlanRequest {
            schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_string(),
            request_id: "plan:worker:disable:0002".to_string(),
            assignment_generation: disable_request.assignment_generation,
            capabilities_digest: disable_request.capabilities_digest.clone(),
            scope: disable_request.scope.clone(),
            package_id: disable_request.package_id.clone(),
            expected_package_generation: disabled.state.package_generation.unwrap(),
            enabled: false,
        })
        .await
        .unwrap();
    assert_eq!(no_change.status, PluginHostEnablementPlanStatus::NoChange);
    assert!(no_change.plan.is_none());

    let enable_request = PluginHostEnablementPlanRequest {
        schema: PLUGIN_HOST_ENABLEMENT_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:worker:enable:0001".to_owned(),
        assignment_generation: disable_request.assignment_generation,
        capabilities_digest: disable_request.capabilities_digest.clone(),
        scope: disable_request.scope.clone(),
        package_id: disable_request.package_id.clone(),
        expected_package_generation: disabled.state.package_generation.unwrap(),
        enabled: true,
    };
    let planned_enable = recovered
        .plan_enablement(enable_request.clone())
        .await
        .unwrap();
    assert_eq!(
        planned_enable.status,
        PluginHostEnablementPlanStatus::Planned
    );
    let enable_plan = planned_enable.plan.as_ref().unwrap();
    let requests_before_diagnostic = server.requests().len();
    let authorizations_before_diagnostic = authorization_count.load(Ordering::SeqCst);
    let enable_diagnostic = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            "acme/worker",
            "--scope-kind",
            "workspace",
            "--scope-id",
            MANAGED_SCOPE_ID,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(enable_diagnostic.status.success(), "{enable_diagnostic:?}");
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    assert_eq!(
        authorization_count.load(Ordering::SeqCst),
        authorizations_before_diagnostic
    );
    let enable_diagnostic = json(&enable_diagnostic);
    let enable_diagnostic = &enable_diagnostic["data"]["diagnostic"];
    assert_eq!(
        enable_diagnostic["operation"]["operationId"],
        enable_plan.plan.operation_id
    );
    assert_eq!(
        enable_diagnostic["operation"]["planDigest"],
        enable_plan.plan_digest
    );
    assert_eq!(enable_diagnostic["operation"]["action"], "enable");
    assert_eq!(enable_diagnostic["operation"]["phase"], "planned");
    assert_eq!(enable_diagnostic["operation"]["lifecycleUnitCount"], 1);
    assert_eq!(
        enable_diagnostic["operation"]["observedLifecycleUnitCount"],
        0
    );
    assert_eq!(
        enable_diagnostic["operation"]["providerCount"],
        enable_plan.plan.providers.len()
    );
    assert!(enable_diagnostic["operation"]["providers"]
        .as_array()
        .unwrap()
        .iter()
        .all(|provider| provider["readiness"] == "selected"));
    assert_eq!(
        enable_diagnostic["operation"]["recovery"],
        "review-and-apply-exact-plan"
    );
    let enable_apply_request = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:worker:enable:0001".to_owned(),
        assignment_generation: enable_request.assignment_generation,
        capabilities_digest: enable_request.capabilities_digest.clone(),
        scope: enable_request.scope.clone(),
        package_id: enable_request.package_id.clone(),
        operation_id: enable_plan.plan.operation_id.clone(),
        plan_digest: enable_plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: enable_plan.plan.operation_id.clone(),
            plan_digest: enable_plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: enable_plan.plan.created_at_ms + 1,
        }),
    };
    let enable_observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:worker:enable:0001".to_owned(),
        assignment_generation: enable_request.assignment_generation,
        capabilities_digest: enable_request.capabilities_digest,
        scope: enable_request.scope,
        package_id: enable_request.package_id,
        operation_id: enable_plan.plan.operation_id.clone(),
        plan_digest: enable_plan.plan_digest.clone(),
    };
    let enabled = apply_enablement_through_observed_windows(
        &recovered,
        &home,
        enable_apply_request,
        enable_observation,
    )
    .await;
    assert_eq!(enabled.state.desired, PluginDesiredState::Enabled);
    assert!(enabled.state.package_generation.unwrap() > disabled.state.package_generation.unwrap());
    assert_eq!(authorization_count.load(Ordering::SeqCst), 0);

    let scope_digest = recovered.managed_scope().descriptor_digest().unwrap();
    let request_path = home
        .join("state/plugin-host-manager")
        .join(scope_digest.strip_prefix("sha256:").unwrap())
        .join("requests")
        .join(format!("{:x}.json", Sha256::digest(b"plan:worker:0001")));
    let mut stale = observe_request;
    stale.request_id = "observe:worker:stale".to_string();
    stale.scope.fence_generation -= 1;
    let error = recovered.observe(stale).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.managed_scope_fence_mismatch");

    let mut tampered: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&request_path).unwrap()).unwrap();
    tampered["recordDigest"] = serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    std::fs::write(&request_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let error = recovered.apply(apply_request).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.host_store_invalid");
}

async fn apply_enablement_through_observed_windows(
    host: &CognitivePackageHostManager,
    home: &std::path::Path,
    apply_request: PluginHostApplyRequest,
    observation: PluginHostOperationObservationRequest,
) -> a3s_use_core::PluginHostApplyResult {
    assert_eq!(observation.scope, apply_request.scope);
    assert_eq!(observation.package_id, apply_request.package_id);
    assert_eq!(observation.operation_id, apply_request.operation_id);
    assert_eq!(observation.plan_digest, apply_request.plan_digest);

    let planned = host.observe_operation(observation.clone()).await.unwrap();
    assert_eq!(
        planned.status.phase,
        PluginHostOperationPhase::AwaitingConfirmation
    );
    assert_eq!(
        planned.status.cancellability,
        PluginHostOperationCancellability::Cancellable
    );

    let lifecycle_generation =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")))
            .get(apply_request.package_id.as_str())
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation
            .unwrap();
    let managed_scope_digest = apply_request.scope.descriptor_digest().unwrap();
    let plan_scope = apply_request.scope.plan_scope();
    let lifecycle_path = home
        .join("state/operations/plugins")
        .join(plan_scope.kind.as_str())
        .join(format!("{:x}", Sha256::digest(plan_scope.id.as_bytes())))
        .join(apply_request.package_id.as_str())
        .join("active.json");
    let enablement_scope_digest = format!(
        "{:x}",
        Sha256::digest(format!("{}\n{}", plan_scope.kind.as_str(), plan_scope.id).as_bytes())
    );
    let enablement_state_path = home
        .join("state/package-enablement/scopes")
        .join(&enablement_scope_digest)
        .join(apply_request.package_id.as_str())
        .join("state.json");
    let enablement_operation_path = home
        .join("state/package-enablement/operations")
        .join(&enablement_scope_digest)
        .join(format!(
            "{:x}.json",
            Sha256::digest(apply_request.operation_id.as_bytes())
        ));

    let lifecycle_lock = exclusive_lock(&lifecycle_path.with_file_name(".operation.lock"));
    let admission_host = host.clone();
    let admission_apply = apply_request.clone();
    let admission_attempt =
        tokio::spawn(async move { admission_host.apply(admission_apply).await });
    let mut reached_admission = false;
    for _ in 0..500 {
        let active_state = std::fs::read(&enablement_state_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|state| {
                state["active"]["request"]["operationId"] == apply_request.operation_id.as_str()
            });
        if active_state {
            reached_admission = true;
            break;
        }
        if admission_attempt.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        reached_admission,
        "enablement apply did not persist admission before its lifecycle journal"
    );
    admission_attempt.abort();
    let _ = admission_attempt.await;
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);
    let preparing = tokio::time::timeout(
        Duration::from_secs(5),
        host.observe_operation(observation.clone()),
    )
    .await
    .expect("operation observation blocked after admission interruption")
    .unwrap();
    assert_eq!(preparing.status.phase, PluginHostOperationPhase::Preparing);
    assert_eq!(
        preparing.status.cancellability,
        PluginHostOperationCancellability::TooLate
    );
    assert!(preparing.status.progress.is_none());

    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks")
            .join(apply_request.package_id.as_str())
            .join(format!("{lifecycle_generation:020}.lock")),
    );
    let host_store_lock = exclusive_lock(
        &home
            .join("state/plugin-host-manager")
            .join(managed_scope_digest.strip_prefix("sha256:").unwrap())
            .join(".store.lock"),
    );
    let interrupted_host = host.clone();
    let interrupted_apply = apply_request.clone();
    let interrupted = tokio::spawn(async move { interrupted_host.apply(interrupted_apply).await });
    let mut reached_publication = false;
    for _ in 0..500 {
        let active_state = std::fs::read(&enablement_state_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|state| {
                state["active"]["request"]["operationId"] == apply_request.operation_id.as_str()
            });
        let active_lifecycle = std::fs::read(&lifecycle_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|operation| {
                operation["intent"]["operationId"] == apply_request.operation_id.as_str()
                    && operation["status"] == "applying"
            });
        if active_state && active_lifecycle {
            reached_publication = true;
            break;
        }
        if interrupted.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        reached_publication,
        "enablement apply did not retain active state and lifecycle evidence before publication"
    );
    let publishing = tokio::time::timeout(
        Duration::from_secs(5),
        host.observe_operation(observation.clone()),
    )
    .await
    .expect("operation observation blocked behind the live apply")
    .unwrap();
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);
    assert_eq!(
        publishing.status.phase,
        PluginHostOperationPhase::Publishing
    );
    assert_eq!(
        publishing.status.cancellability,
        PluginHostOperationCancellability::TooLate
    );
    let progress = publishing.status.progress.unwrap();
    assert!(progress.completed_steps < progress.total_steps);
    assert!(progress.current_surface.is_none());

    let mut reached_enablement_completion = false;
    for _ in 0..500 {
        let state_completed = std::fs::read(&enablement_state_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|state| {
                state.get("active").is_none_or(serde_json::Value::is_null)
                    && state["enabled"] == apply_request.operation_id.starts_with("enable:")
            });
        if enablement_operation_path.is_file() && state_completed {
            reached_enablement_completion = true;
            break;
        }
        if interrupted.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        reached_enablement_completion,
        "enablement apply did not persist its result before the Host outcome"
    );
    let completed_use_diagnostic = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            apply_request.package_id.as_str(),
            "--scope-kind",
            apply_request.scope.scope_kind.as_str(),
            "--scope-id",
            &apply_request.scope.scope_id,
            "--json",
        ])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap();
    assert!(
        !completed_use_diagnostic.status.success(),
        "{completed_use_diagnostic:?}"
    );
    assert_eq!(
        json(&completed_use_diagnostic)["error"]["code"],
        "use.plugin.operation_diagnostic_not_found"
    );
    let finalizing = tokio::time::timeout(
        Duration::from_secs(5),
        host.observe_operation(observation.clone()),
    )
    .await
    .expect("operation observation blocked behind Host outcome persistence")
    .unwrap();
    FileExt::unlock(&host_store_lock).unwrap();
    drop(host_store_lock);
    assert_eq!(
        finalizing.status.phase,
        PluginHostOperationPhase::Finalizing
    );
    assert_eq!(
        finalizing.status.cancellability,
        PluginHostOperationCancellability::TooLate
    );

    let applied = interrupted.await.unwrap().unwrap();
    let completed = host.observe_operation(observation).await.unwrap();
    assert_eq!(completed.status.phase, PluginHostOperationPhase::Completed);
    assert_eq!(
        completed.status.cancellability,
        PluginHostOperationCancellability::NotApplicable
    );
    assert_eq!(
        completed.status.operation_result_digest,
        Some(applied.operation_result_digest.clone())
    );
    assert_eq!(completed.status.state, Some(applied.state.clone()));
    applied
}
