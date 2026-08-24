use super::*;

#[tokio::test]
async fn gateway_bind_failure_replays_durable_provisioning_without_a_second_runtime_effect() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent_generation(&manifest, 31, PluginLifecycleAction::Install);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let readiness = Arc::new(RecordingReadiness {
        fail_tool_binds: AtomicUsize::new(1),
        ..RecordingReadiness::default()
    });
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection.clone(),
        registry.clone(),
        store.clone(),
        readiness.clone(),
    );
    let prepare_key = key(&intent, PluginSurfaceKind::Tool, &tool.id);

    let error = host
        .prepare_tool(&intent, tool, prepare_key)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.gateway_bind_failed");
    assert!(store
        .get_generation(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
    let pending = store
        .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        pending.phase,
        RuntimeServiceProvisioningPhase::RuntimeApplied
    );
    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);

    let restarted = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry,
        store.clone(),
        readiness.clone(),
    );
    restarted
        .prepare_tool(&intent, tool, prepare_key)
        .await
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 2);
    assert!(store
        .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        store
            .get_generation(&intent.scope, &plan.surface(), intent.generation)
            .await
            .unwrap(),
        Some(RuntimeBindingReceipt::Service(_))
    ));
}

#[tokio::test]
async fn mcp_initialize_bind_failure_replays_the_exact_pending_generation() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent_generation(&manifest, 33, PluginLifecycleAction::Install);
    let mcp = manifest
        .mcp_servers
        .iter()
        .find(|surface| matches!(&surface.launch, PluginMcpLaunch::StreamableHttp { .. }))
        .unwrap();
    let plan = mcp_plan(&intent, mcp);
    let unused_tool = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], unused_tool, runtime.clone()).await;
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let readiness = Arc::new(RecordingReadiness {
        fail_mcp_binds: AtomicUsize::new(1),
        ..RecordingReadiness::default()
    });
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry,
        store.clone(),
        readiness.clone(),
    );
    let prepare_key = key(&intent, PluginSurfaceKind::Mcp, &mcp.id);

    let error = host
        .prepare_mcp(&intent, mcp, prepare_key)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.gateway_bind_failed");
    assert_eq!(
        store
            .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
            .await
            .unwrap()
            .unwrap()
            .phase,
        RuntimeServiceProvisioningPhase::RuntimeApplied
    );

    host.prepare_mcp(&intent, mcp, prepare_key).await.unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 2);
    assert!(store
        .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
    assert!(matches!(
        store
            .get_generation(&intent.scope, &plan.surface(), intent.generation)
            .await
            .unwrap(),
        Some(RuntimeBindingReceipt::Service(receipt))
            if matches!(
                receipt.readiness,
                RuntimeServiceReadinessEvidence::McpInitialized { .. }
            )
    ));
}

#[tokio::test]
async fn candidate_rollback_finishes_and_removes_an_interrupted_service_provisioning() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent_generation(&manifest, 32, PluginLifecycleAction::Install);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let readiness = Arc::new(RecordingReadiness {
        fail_tool_binds: AtomicUsize::new(1),
        ..RecordingReadiness::default()
    });
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry,
        store.clone(),
        readiness.clone(),
    );

    host.prepare_tool(
        &intent,
        tool,
        key(&intent, PluginSurfaceKind::Tool, &tool.id),
    )
    .await
    .unwrap_err();
    host.remove_tool(&intent, tool, "candidate-rollback-tool")
        .await
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 2);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 1);
    assert!(store
        .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_generation(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn candidate_rollback_drops_a_pre_apply_marker_without_starting_a_service() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent_generation(&manifest, 34, PluginLifecycleAction::Install);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let prepare_key = key(&intent, PluginSurfaceKind::Tool, &tool.id);
    let selected = &selection.surfaces()[0];
    let pending = RuntimeServiceProvisioningReceipt::from_plan(
        selected.plan(),
        selected.provider(),
        prepare_key,
        request_id("apply-tool", prepare_key),
    )
    .unwrap();
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    store.put_provisioning(&pending).await.unwrap();
    let readiness = Arc::new(RecordingReadiness::default());
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry,
        store.clone(),
        readiness.clone(),
    );

    host.remove_tool(&intent, tool, "candidate-rollback-before-apply")
        .await
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 0);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 0);
    assert!(store
        .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn restart_reconciles_a_synced_binding_left_with_its_provisioning_receipt() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent_generation(&manifest, 35, PluginLifecycleAction::Install);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let prepare_key = key(&intent, PluginSurfaceKind::Tool, &tool.id);
    let selected = &selection.surfaces()[0];
    let mut pending = RuntimeServiceProvisioningReceipt::from_plan(
        selected.plan(),
        selected.provider(),
        prepare_key,
        request_id("apply-tool", prepare_key),
    )
    .unwrap();
    let activation = selected
        .client()
        .apply_service(
            selected.plan(),
            selected.provider(),
            pending.apply_request_id.clone(),
            None,
        )
        .await
        .unwrap();
    pending
        .record_runtime_observation(
            selected.plan(),
            selected.provider(),
            activation.observation().clone(),
        )
        .unwrap();
    pending
        .record_gateway_readiness(
            RuntimeEndpointRef::parse(endpoint_id(&intent, &tool.id, prepare_key)).unwrap(),
            RuntimeServiceReadinessEvidence::HttpHealthy,
        )
        .unwrap();
    let binding = RuntimeBindingReceipt::Service(pending.binding_receipt().unwrap());
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    store.put_provisioning(&pending).await.unwrap();
    store.put(&binding).await.unwrap();
    let readiness = Arc::new(RecordingReadiness::default());
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry,
        store.clone(),
        readiness.clone(),
    );

    host.prepare_tool(&intent, tool, prepare_key).await.unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 0);
    assert!(store
        .get_provisioning(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
    assert_eq!(
        store
            .get_generation(&intent.scope, &plan.surface(), intent.generation)
            .await
            .unwrap(),
        Some(binding)
    );
}
