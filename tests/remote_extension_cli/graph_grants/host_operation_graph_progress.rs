use super::*;

#[tokio::test]
async fn host_graph_operation_observation_aggregates_dependency_progress() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let dependency = PluginPackageDependency::new("acme/base", "^1.0.0").unwrap();
    let mut targets = cognitive_tool_targets_version_with_dependencies_and_payload(
        &temporary.path().join("root"),
        "acme/worker",
        "worker-graph-progress",
        "1.0.0",
        &target,
        vec![dependency],
        0,
    );
    targets.extend(cognitive_tool_targets_version(
        &temporary.path().join("base"),
        "acme/base",
        "base-graph-progress",
        "1.0.0",
        &target,
    ));
    targets.extend(
        cognitive_tool_targets_version_with_dependencies_and_payload(
            &temporary.path().join("root-v2"),
            "acme/worker",
            "worker-graph-progress",
            "2.0.0",
            &target,
            vec![PluginPackageDependency::new("acme/base", "^2.0.0").unwrap()],
            0,
        ),
    );
    targets.extend(cognitive_tool_targets_version(
        &temporary.path().join("base-v2"),
        "acme/base",
        "base-graph-progress",
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 70, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temporary.path().join("graph-progress-host-home");
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
        host_id: "host:graph-progress".to_owned(),
        scope_kind: PlanScopeKind::Workspace,
        scope_id: "workspace:graph-progress".to_owned(),
        authority_id: "graph-progress:user".to_owned(),
        fence_generation: 1,
        fence_digest: format!("sha256:{}", "7".repeat(64)),
    };
    let host = CognitivePackageHostManager::new(
        scope.clone(),
        "use:graph-progress-test",
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
                query: "worker".to_owned(),
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
        .find(|candidate| {
            candidate.record.package_id == "acme/worker" && candidate.record.version == "1.0.0"
        })
        .unwrap();
    let lock = host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &candidate)
        .await
        .unwrap();
    assert_eq!(lock.packages.len(), 2);
    let capabilities = host.capabilities().await.unwrap();
    let capabilities_digest = capabilities.descriptor_digest().unwrap();
    let plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:graph-progress:0001".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        action: PluginOperationAction::Install,
        package_id: PluginPackageId::parse("acme/worker").unwrap(),
        candidate: Some(candidate),
        package_lock: Some(lock),
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        }],
    };
    let planned = host.plan(plan_request.clone()).await.unwrap();
    assert_eq!(planned.plan.plan.packages.len(), 2);
    let apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:graph-progress:0001".to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest: capabilities_digest.clone(),
        scope: scope.clone(),
        package_id: plan_request.package_id.clone(),
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
    let observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:graph-progress:0001".to_owned(),
        assignment_generation: plan_request.assignment_generation,
        capabilities_digest,
        scope: scope.clone(),
        package_id: plan_request.package_id,
        operation_id: planned.plan.plan.operation_id.clone(),
        plan_digest: planned.plan.plan_digest.clone(),
    };

    let next_generation = ExtensionRegistry::new(paths)
        .snapshot()
        .await
        .unwrap()
        .generation
        + 1;
    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks/acme/worker")
            .join(format!("{next_generation:020}.lock")),
    );
    let applying_host = host.clone();
    let applying = tokio::spawn(async move { applying_host.apply(apply).await });
    let plan_scope = scope.plan_scope();
    let lifecycle_root = home
        .join("state/operations/plugins")
        .join(plan_scope.kind.as_str())
        .join(format!("{:x}", Sha256::digest(plan_scope.id.as_bytes())));
    let lifecycle_paths = ["acme/base", "acme/worker"]
        .map(|package_id| lifecycle_root.join(package_id).join("active.json"));
    let mut graph_prepared = false;
    for _ in 0..500 {
        graph_prepared = lifecycle_paths.iter().all(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|operation| {
                    operation["intent"]["operationId"] == planned.plan.plan.operation_id.as_str()
                        && operation["status"] == "applying"
                        && operation["receipts"]
                            .as_array()
                            .is_some_and(|receipts| receipts.len() == 2)
                })
        });
        if graph_prepared || applying.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        graph_prepared,
        "the two-node graph did not reach its atomic publication boundary"
    );
    let observed = tokio::time::timeout(
        Duration::from_secs(5),
        host.observe_operation(observation.clone()),
    )
    .await
    .expect("graph operation observation blocked behind publication")
    .unwrap();
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);
    assert_eq!(observed.status.phase, PluginHostOperationPhase::Publishing);
    assert_eq!(
        observed.status.cancellability,
        PluginHostOperationCancellability::TooLate
    );
    let progress = observed.status.progress.unwrap();
    assert_eq!(progress.completed_steps, 4);
    assert_eq!(progress.total_steps, 6);
    assert!(progress.current_surface.is_none());

    let applied = applying.await.unwrap().unwrap();
    let completed = host.observe_operation(observation).await.unwrap();
    assert_eq!(completed.status.phase, PluginHostOperationPhase::Completed);
    assert_eq!(
        completed.status.operation_result_digest,
        Some(applied.operation_result_digest)
    );

    let upgrade_candidate = host
        .search_cognitive_packages(
            CognitiveRegistryAccess::Refreshed,
            None,
            &PluginCatalogSearch {
                query: "worker".to_owned(),
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
        .find(|candidate| {
            candidate.record.package_id == "acme/worker" && candidate.record.version == "2.0.0"
        })
        .unwrap();
    let upgrade_lock = host
        .resolve_cognitive_package_lock(CognitiveRegistryAccess::Refreshed, &upgrade_candidate)
        .await
        .unwrap();
    let upgrade_plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:graph-progress:upgrade:0001".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: scope.clone(),
        action: PluginOperationAction::Upgrade,
        package_id: PluginPackageId::parse("acme/worker").unwrap(),
        candidate: Some(upgrade_candidate),
        package_lock: Some(upgrade_lock.clone()),
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Tool,
            id: "convert".to_owned(),
        }],
    };
    let planned_upgrade = host.plan(upgrade_plan_request.clone()).await.unwrap();
    assert!(planned_upgrade
        .plan
        .plan
        .packages
        .iter()
        .all(|transition| { transition.change == a3s_use_core::PlanPackageChangeKind::Replace }));
    let upgrade_apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:graph-progress:upgrade:0001".to_owned(),
        assignment_generation: upgrade_plan_request.assignment_generation,
        capabilities_digest: upgrade_plan_request.capabilities_digest.clone(),
        scope: scope.clone(),
        package_id: upgrade_plan_request.package_id.clone(),
        operation_id: planned_upgrade.plan.plan.operation_id.clone(),
        plan_digest: planned_upgrade.plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: planned_upgrade.plan.plan.operation_id.clone(),
            plan_digest: planned_upgrade.plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: planned_upgrade.plan.plan.created_at_ms + 1,
        }),
    };
    let upgrade_observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:graph-progress:upgrade:0001".to_owned(),
        assignment_generation: upgrade_plan_request.assignment_generation,
        capabilities_digest: upgrade_plan_request.capabilities_digest,
        scope: scope.clone(),
        package_id: upgrade_plan_request.package_id,
        operation_id: planned_upgrade.plan.plan.operation_id.clone(),
        plan_digest: planned_upgrade.plan.plan_digest.clone(),
    };
    let scope_digest = scope.descriptor_digest().unwrap();
    let host_store_lock = exclusive_lock(
        &home
            .join("state/plugin-host-manager")
            .join(scope_digest.strip_prefix("sha256:").unwrap())
            .join(".store.lock"),
    );
    let upgrading_host = host.clone();
    let upgrading = tokio::spawn(async move { upgrading_host.apply(upgrade_apply).await });
    let retirement_paths = ["acme/base", "acme/worker"].map(|package_id| {
        (
            lifecycle_root.join(package_id).join("active.json"),
            lifecycle_root.join(package_id).join("last.json"),
        )
    });
    let mut replacement_lifecycles_completed = false;
    // The replacement journals are written by two independent lifecycle
    // workers. A cold CI runner can spend several seconds starting the
    // workers before either journal becomes visible, so keep the polling
    // budget comfortably above the normal path without hiding a stuck apply.
    for _ in 0..3_000 {
        replacement_lifecycles_completed = retirement_paths.iter().all(|(active, previous)| {
            let active = std::fs::read(active)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            let previous = std::fs::read(previous)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok());
            active.is_some_and(|operation| {
                operation["intent"]["operationId"]
                    == planned_upgrade.plan.plan.operation_id.as_str()
                    && operation["intent"]["action"] == "uninstall"
                    && operation["status"] == "completed"
                    && operation["receipts"]
                        .as_array()
                        .is_some_and(|receipts| receipts.len() == 4)
            }) && previous.is_some_and(|operation| {
                operation["intent"]["operationId"]
                    == planned_upgrade.plan.plan.operation_id.as_str()
                    && operation["intent"]["action"] == "upgrade"
                    && operation["status"] == "completed"
                    && operation["receipts"]
                        .as_array()
                        .is_some_and(|receipts| receipts.len() == 3)
            })
        });
        if replacement_lifecycles_completed || upgrading.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        replacement_lifecycles_completed,
        "the two-node replacement did not persist candidate and retirement journals"
    );
    let finalizing = tokio::time::timeout(
        Duration::from_secs(5),
        host.observe_operation(upgrade_observation.clone()),
    )
    .await
    .expect("upgrade observation blocked behind Host outcome persistence")
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
    let progress = finalizing.status.progress.unwrap();
    assert_eq!(progress.completed_steps, 14);
    assert_eq!(progress.total_steps, 14);
    assert!(progress.current_surface.is_none());

    let upgraded = upgrading.await.unwrap().unwrap();
    let completed_upgrade = host.observe_operation(upgrade_observation).await.unwrap();
    assert_eq!(
        completed_upgrade.status.phase,
        PluginHostOperationPhase::Completed
    );
    assert_eq!(
        completed_upgrade.status.operation_result_digest,
        Some(upgraded.operation_result_digest)
    );

    let uninstall_plan_request = PluginHostPlanRequest {
        schema: PLUGIN_HOST_PLAN_REQUEST_SCHEMA.to_owned(),
        request_id: "plan:graph-progress:uninstall:0001".to_owned(),
        assignment_generation: 1,
        capabilities_digest: capabilities.descriptor_digest().unwrap(),
        scope: scope.clone(),
        action: PluginOperationAction::Uninstall,
        package_id: PluginPackageId::parse("acme/worker").unwrap(),
        candidate: None,
        package_lock: Some(upgrade_lock),
        selected_surfaces: Vec::new(),
    };
    let planned_uninstall = host.plan(uninstall_plan_request.clone()).await.unwrap();
    assert!(planned_uninstall
        .plan
        .plan
        .packages
        .iter()
        .all(|transition| { transition.change == a3s_use_core::PlanPackageChangeKind::Remove }));
    let uninstall_apply = PluginHostApplyRequest {
        schema: PLUGIN_HOST_APPLY_REQUEST_SCHEMA.to_owned(),
        request_id: "apply:graph-progress:uninstall:0001".to_owned(),
        assignment_generation: uninstall_plan_request.assignment_generation,
        capabilities_digest: uninstall_plan_request.capabilities_digest.clone(),
        scope: scope.clone(),
        package_id: uninstall_plan_request.package_id.clone(),
        operation_id: planned_uninstall.plan.plan.operation_id.clone(),
        plan_digest: planned_uninstall.plan.plan_digest.clone(),
        confirmation: Some(PluginOperationConfirmation {
            schema: PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_owned(),
            operation_id: planned_uninstall.plan.plan.operation_id.clone(),
            plan_digest: planned_uninstall.plan.plan_digest.clone(),
            confirmed_by: PlanActor::User,
            confirmed_at_ms: planned_uninstall.plan.plan.created_at_ms + 1,
        }),
    };
    let uninstall_observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:graph-progress:uninstall:0001".to_owned(),
        assignment_generation: uninstall_plan_request.assignment_generation,
        capabilities_digest: uninstall_plan_request.capabilities_digest,
        scope: scope.clone(),
        package_id: uninstall_plan_request.package_id,
        operation_id: planned_uninstall.plan.plan.operation_id.clone(),
        plan_digest: planned_uninstall.plan.plan_digest.clone(),
    };
    let host_store_lock = exclusive_lock(
        &home
            .join("state/plugin-host-manager")
            .join(scope_digest.strip_prefix("sha256:").unwrap())
            .join(".store.lock"),
    );
    let uninstalling_host = host.clone();
    let uninstalling = tokio::spawn(async move { uninstalling_host.apply(uninstall_apply).await });
    let mut uninstall_lifecycles_completed = false;
    for _ in 0..500 {
        uninstall_lifecycles_completed = lifecycle_paths.iter().all(|path| {
            std::fs::read(path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
                .is_some_and(|operation| {
                    operation["intent"]["operationId"]
                        == planned_uninstall.plan.plan.operation_id.as_str()
                        && operation["intent"]["action"] == "uninstall"
                        && operation["status"] == "completed"
                        && operation["receipts"]
                            .as_array()
                            .is_some_and(|receipts| receipts.len() == 4)
                })
        });
        if uninstall_lifecycles_completed || uninstalling.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        uninstall_lifecycles_completed,
        "the two-node uninstall did not complete every package lifecycle"
    );
    let finalizing_uninstall = tokio::time::timeout(
        Duration::from_secs(5),
        host.observe_operation(uninstall_observation.clone()),
    )
    .await
    .expect("uninstall observation blocked behind Host outcome persistence")
    .unwrap();
    FileExt::unlock(&host_store_lock).unwrap();
    drop(host_store_lock);
    assert_eq!(
        finalizing_uninstall.status.phase,
        PluginHostOperationPhase::Finalizing
    );
    let progress = finalizing_uninstall.status.progress.unwrap();
    assert_eq!(progress.completed_steps, 8);
    assert_eq!(progress.total_steps, 8);

    let uninstalled = uninstalling.await.unwrap().unwrap();
    assert_eq!(uninstalled.state.desired, PluginDesiredState::Absent);
    let completed_uninstall = host.observe_operation(uninstall_observation).await.unwrap();
    assert_eq!(
        completed_uninstall.status.phase,
        PluginHostOperationPhase::Completed
    );
    assert_eq!(
        completed_uninstall.status.operation_result_digest,
        Some(uninstalled.operation_result_digest)
    );
}
