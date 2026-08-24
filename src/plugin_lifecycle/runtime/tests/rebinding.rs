use super::*;

#[tokio::test]
async fn confirmed_provider_loss_rebinds_gateway_without_removing_runtime_generation() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let readiness = Arc::new(RecordingReadiness::default());
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
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
    .unwrap();
    let original = service_receipt(&store, &intent, &plan).await;
    runtime.simulate_confirmed_provider_loss();

    host.prepare_tool(&intent, tool, &format!("sha256:{}", "c".repeat(64)))
        .await
        .unwrap();
    let rebound = service_receipt(&store, &intent, &plan).await;

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 0);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 2);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 1);
    assert_ne!(rebound.endpoint_ref, original.endpoint_ref);
    assert_eq!(
        *readiness.removed_endpoints.lock().unwrap(),
        vec![original.endpoint_ref.as_str().to_string()]
    );
    assert_eq!(rebound.unit_id, original.unit_id);
    assert_eq!(rebound.generation, original.generation);
    assert!(rebound.runtime_started_at_ms > original.runtime_started_at_ms);
    assert!(rebound.observation_revision > original.observation_revision);

    host.remove_tool(&intent, tool, "remove-rebound-tool")
        .await
        .unwrap();
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 2);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 2);
    assert_eq!(
        *readiness.removed_endpoints.lock().unwrap(),
        vec![
            original.endpoint_ref.as_str().to_string(),
            rebound.endpoint_ref.as_str().to_string(),
        ]
    );
}

#[tokio::test]
async fn same_generation_provider_restart_rebinds_gateway_without_removing_runtime_generation() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let readiness = Arc::new(RecordingReadiness::default());
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
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
    .unwrap();
    let original = service_receipt(&store, &intent, &plan).await;
    runtime.simulate_same_generation_provider_restart();

    host.prepare_tool(&intent, tool, &format!("sha256:{}", "e".repeat(64)))
        .await
        .unwrap();
    let rebound = service_receipt(&store, &intent, &plan).await;

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 0);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 1);
    assert_ne!(rebound.endpoint_ref, original.endpoint_ref);
    assert_eq!(
        *readiness.removed_endpoints.lock().unwrap(),
        vec![original.endpoint_ref.as_str().to_string()]
    );
    assert_eq!(rebound.unit_id, original.unit_id);
    assert_eq!(rebound.generation, original.generation);
    assert!(rebound.runtime_started_at_ms > original.runtime_started_at_ms);
    assert!(rebound.observation_revision > original.observation_revision);
}

#[tokio::test]
async fn interrupted_route_detach_preserves_runtime_and_binding_for_exact_replay() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let plan = tool_plan(&intent, tool);
    let runtime = Arc::new(FakeRuntime::new(capabilities(&plan, "tool-runtime")));
    let unused_mcp = Arc::new(FakeRuntime::new(capabilities(&plan, "mcp-runtime")));
    let (selection, registry) = selection(vec![plan.clone()], runtime.clone(), unused_mcp).await;
    let readiness = Arc::new(RecordingReadiness::default());
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry,
        store.clone(),
        readiness.clone(),
    );
    let prepare_key = format!("sha256:{}", "d".repeat(64));

    host.prepare_tool(
        &intent,
        tool,
        key(&intent, PluginSurfaceKind::Tool, &tool.id),
    )
    .await
    .unwrap();
    let original = service_receipt(&store, &intent, &plan).await;
    runtime.simulate_confirmed_provider_loss();
    readiness.fail_removals.store(1, Ordering::SeqCst);

    let error = host
        .prepare_tool(&intent, tool, &prepare_key)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.gateway_remove_failed");
    assert_eq!(service_receipt(&store, &intent, &plan).await, original);
    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 0);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 1);
    assert!(readiness.removed_endpoints.lock().unwrap().is_empty());

    host.prepare_tool(&intent, tool, &prepare_key)
        .await
        .unwrap();
    let rebound = service_receipt(&store, &intent, &plan).await;
    assert_ne!(rebound.endpoint_ref, original.endpoint_ref);
    assert!(rebound.observation_revision > original.observation_revision);
    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 0);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 2);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 2);
    assert_eq!(
        *readiness.removed_endpoints.lock().unwrap(),
        vec![original.endpoint_ref.as_str().to_string()]
    );
}

impl FakeRuntime {
    fn simulate_confirmed_provider_loss(&self) {
        let mut observation = self.observation.lock().unwrap();
        let observation = observation
            .as_mut()
            .expect("the test Runtime Service must already be running");
        observation.state = RuntimeUnitState::Unknown;
        observation.observed_at_ms += 50;
        observation.finished_at_ms = None;
        observation.health = None;
        observation.failure = None;
        observation.clear_service_endpoints();
    }

    fn simulate_same_generation_provider_restart(&self) {
        let mut observation = self.observation.lock().unwrap();
        let observation = observation
            .as_mut()
            .expect("the test Runtime Service must already be running");
        observation.observed_at_ms += 1_000;
        observation.started_at_ms = Some(observation.observed_at_ms - 100);
        observation.provider_resource_id = Some("resource-restarted".to_string());
        observation.health = Some(RuntimeHealthObservation {
            state: RuntimeHealthState::Healthy,
            checked_at_ms: observation.observed_at_ms,
            message: None,
        });
    }
}

async fn service_receipt(
    store: &RuntimeBindingStore,
    intent: &PluginLifecycleIntent,
    plan: &RuntimeSurfacePlan,
) -> crate::plugin_runtime::RuntimeServiceBindingReceipt {
    match store
        .get_generation(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .unwrap()
    {
        RuntimeBindingReceipt::Service(receipt) => receipt,
        receipt => panic!("expected Runtime Service receipt, got {receipt:?}"),
    }
}
