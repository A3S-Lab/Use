use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a3s_runtime::contract::{
    ArtifactRef, HealthCheckKind, IsolationLevel, MountKind, NetworkMode, ResourceControl,
    RuntimeActionRequest, RuntimeApplyRequest, RuntimeCapabilities, RuntimeEvidence,
    RuntimeExecRequest, RuntimeExecResult, RuntimeFeature, RuntimeHealthObservation,
    RuntimeHealthState, RuntimeInspection, RuntimeLogChunk, RuntimeLogQuery, RuntimeObservation,
    RuntimeRemoval, RuntimeServiceEndpoint, RuntimeUnitClass, RuntimeUnitState,
};
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeError, RuntimeProviderFactory,
    RuntimeResult,
};
use a3s_use_core::{
    McpReleaseDescriptor, PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef,
    ToolReleaseDescriptor, UseError, UseResult,
};
use a3s_use_extension::{ExtensionManifest, PluginMcpLaunch, ToolTaskSource, ToolWorkload};
use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::{PluginLifecycleAction, PluginLifecycleIntentSpec};
use crate::plugin_runtime::{
    plan_mcp_service_release, plan_tool_service_release, RuntimeProviderAssignment,
    RuntimeProviderSelector, RuntimeResourcePolicy, RuntimeSurfaceContext, RuntimeWorkloadPolicy,
};

use super::*;

const MANIFEST: &str = include_str!(
    "../../../crates/extension/fixtures/packages/plugin-v3/package/a3s-use-extension.acl"
);
const PACKAGE_DIGEST: &str =
    include_str!("../../../crates/extension/fixtures/packages/plugin-v3/package.sha256");
const TOOL_DESCRIPTOR: &[u8] = include_bytes!(
    "../../../crates/extension/fixtures/packages/plugin-v3/package/releases/index-tool-v1.json"
);
const MCP_DESCRIPTOR: &[u8] = include_bytes!(
    "../../../crates/extension/fixtures/packages/plugin-v3/package/releases/library-mcp-v1.json"
);

#[tokio::test]
async fn native_tool_and_stdio_mcp_remain_static_launchers() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let readiness = Arc::new(RecordingReadiness::default());
    let temporary = tempfile::tempdir().unwrap();
    let host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        RuntimeProviderSelection::default(),
        Arc::new(RuntimeClientRegistry::new()),
        RuntimeBindingStore::new(temporary.path()),
        readiness.clone(),
    );
    let tool = manifest
        .tools
        .iter()
        .find(|surface| {
            matches!(
                &surface.workload,
                ToolWorkload::Task(task)
                    if matches!(&task.source, ToolTaskSource::Executable { .. })
            )
        })
        .unwrap();
    let mcp = manifest
        .mcp_servers
        .iter()
        .find(|surface| matches!(&surface.launch, PluginMcpLaunch::Stdio { .. }))
        .unwrap();

    host.prepare_tool(
        &intent,
        tool,
        key(&intent, PluginSurfaceKind::Tool, &tool.id),
    )
    .await
    .unwrap();
    host.prepare_mcp(&intent, mcp, key(&intent, PluginSurfaceKind::Mcp, &mcp.id))
        .await
        .unwrap();
    host.stop_tool(&intent, tool, "stop-native").await.unwrap();
    host.remove_mcp(&intent, mcp, "remove-stdio").await.unwrap();
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 0);
    for surface in [
        PlanQualifiedSurfaceRef {
            package_id: intent.package_id.clone(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: tool.id.clone(),
            },
        },
        PlanQualifiedSurfaceRef {
            package_id: intent.package_id.clone(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Mcp,
                id: mcp.id.clone(),
            },
        },
    ] {
        assert!(host
            .store()
            .get(&intent.scope, &surface)
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn tool_and_streamable_http_mcp_use_receipt_backed_runtime_lifecycle() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent(&manifest);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let mcp = manifest
        .mcp_servers
        .iter()
        .find(|surface| matches!(&surface.launch, PluginMcpLaunch::StreamableHttp { .. }))
        .unwrap();
    let tool_plan = tool_plan(&intent, tool);
    let mcp_plan = mcp_plan(&intent, mcp);
    let tool_runtime = Arc::new(FakeRuntime::new(capabilities(&tool_plan, "tool-runtime")));
    let mcp_runtime = Arc::new(FakeRuntime::new(capabilities(&mcp_plan, "mcp-runtime")));
    let (selection, registry) = selection(
        vec![tool_plan.clone(), mcp_plan.clone()],
        tool_runtime.clone(),
        mcp_runtime.clone(),
    )
    .await;
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
    let tool_key = key(&intent, PluginSurfaceKind::Tool, &tool.id);
    let mcp_key = key(&intent, PluginSurfaceKind::Mcp, &mcp.id);

    let prepared_tool = host.prepare_tool(&intent, tool, tool_key).await.unwrap();
    let prepared_mcp = host.prepare_mcp(&intent, mcp, mcp_key).await.unwrap();
    assert_eq!(tool_runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 2);
    assert!(matches!(
        store
            .get(&intent.scope, &tool_plan.surface())
            .await
            .unwrap(),
        Some(RuntimeBindingReceipt::Service(_))
    ));
    assert!(matches!(
        store
            .get(&intent.scope, &mcp_plan.surface())
            .await
            .unwrap(),
        Some(RuntimeBindingReceipt::Service(ref receipt))
            if matches!(receipt.readiness, crate::plugin_runtime::RuntimeServiceReadinessEvidence::McpInitialized { .. })
    ));

    let replayed_tool = host.prepare_tool(&intent, tool, tool_key).await.unwrap();
    let replayed_mcp = host.prepare_mcp(&intent, mcp, mcp_key).await.unwrap();
    assert_eq!(replayed_tool, prepared_tool);
    assert_eq!(replayed_mcp, prepared_mcp);
    assert_eq!(tool_runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.calls.load(Ordering::SeqCst), 2);

    let stopped_tool = host.stop_tool(&intent, tool, "disable-tool").await.unwrap();
    assert_eq!(tool_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(tool_runtime.remove_count.load(Ordering::SeqCst), 0);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 0);
    assert!(store
        .get(&intent.scope, &tool_plan.surface())
        .await
        .unwrap()
        .is_some());
    let removed_tool = host
        .remove_tool(&intent, tool, "uninstall-tool")
        .await
        .unwrap();
    let removed_mcp = host
        .remove_mcp(&intent, mcp, "uninstall-mcp")
        .await
        .unwrap();
    assert_eq!(tool_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(tool_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 3);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 2);
    assert!(store
        .get(&intent.scope, &tool_plan.surface())
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(&intent.scope, &mcp_plan.surface())
        .await
        .unwrap()
        .is_none());

    assert_eq!(
        host.remove_tool(&intent, tool, "uninstall-tool")
            .await
            .unwrap(),
        removed_tool
    );
    assert_eq!(
        host.stop_tool(&intent, tool, "disable-tool").await.unwrap(),
        stopped_tool
    );
    assert_eq!(
        host.remove_mcp(&intent, mcp, "uninstall-mcp")
            .await
            .unwrap(),
        removed_mcp
    );
    assert_eq!(tool_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(tool_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.remove_count.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_lifecycle_prepares_next_generation_and_retires_only_the_prior_generation() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let prior_intent = intent_generation(&manifest, 19, PluginLifecycleAction::Install);
    let next_intent = intent_generation(&manifest, 20, PluginLifecycleAction::Upgrade);
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let prior_plan = tool_plan(&prior_intent, tool);
    let next_plan = tool_plan(&next_intent, tool);
    let prior_runtime = Arc::new(FakeRuntime::new(capabilities(&prior_plan, "tool-runtime")));
    let next_runtime = Arc::new(FakeRuntime::new(capabilities(&next_plan, "tool-runtime")));
    let prior_unused_mcp = Arc::new(FakeRuntime::new(capabilities(&prior_plan, "mcp-runtime")));
    let next_unused_mcp = Arc::new(FakeRuntime::new(capabilities(&next_plan, "mcp-runtime")));
    let (prior_selection, prior_registry) = selection(
        vec![prior_plan.clone()],
        prior_runtime.clone(),
        prior_unused_mcp,
    )
    .await;
    let (next_selection, next_registry) = selection(
        vec![next_plan.clone()],
        next_runtime.clone(),
        next_unused_mcp,
    )
    .await;
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let readiness = Arc::new(RecordingReadiness::default());
    let prior_host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        prior_selection,
        prior_registry,
        store.clone(),
        readiness.clone(),
    );
    let next_host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        next_selection,
        next_registry,
        store.clone(),
        readiness,
    );

    prior_host
        .prepare_tool(
            &prior_intent,
            tool,
            key(&prior_intent, PluginSurfaceKind::Tool, &tool.id),
        )
        .await
        .unwrap();
    next_host
        .prepare_tool(
            &next_intent,
            tool,
            key(&next_intent, PluginSurfaceKind::Tool, &tool.id),
        )
        .await
        .unwrap();

    let qualified = prior_plan.surface();
    assert!(store
        .get_generation(&prior_intent.scope, &qualified, prior_intent.generation)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_generation(&next_intent.scope, &qualified, next_intent.generation)
        .await
        .unwrap()
        .is_some());
    prior_host
        .remove_tool(&prior_intent, tool, "retire-prior-tool")
        .await
        .unwrap();

    assert!(store
        .get_generation(&prior_intent.scope, &qualified, prior_intent.generation)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_generation(&next_intent.scope, &qualified, next_intent.generation)
        .await
        .unwrap()
        .is_some());
    assert_eq!(prior_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(prior_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(next_runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(next_runtime.remove_count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn runtime_reenable_replaces_a_stopped_binding_with_new_authorization_semantics() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let prior_intent = intent_generation(&manifest, 23, PluginLifecycleAction::Install);
    let next_intent = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: "runtime-generation-23-reauthorized".to_string(),
            plan_digest: format!("sha256:{}", "2".repeat(64)),
            scope: prior_intent.scope.clone(),
            package_id: prior_intent.package_id.clone(),
            package_digest: prior_intent.package_digest.clone(),
            manifest_digest: prior_intent.manifest_digest.clone(),
            generation: prior_intent.generation,
            action: PluginLifecycleAction::Enable,
            retained_ui_state_surfaces: Vec::new(),
        },
        &manifest,
    )
    .unwrap();
    let tool = manifest
        .tools
        .iter()
        .find(|surface| matches!(&surface.workload, ToolWorkload::Service(_)))
        .unwrap();
    let prior_plan = tool_plan(&prior_intent, tool);
    let next_plan = tool_plan(&next_intent, tool);
    assert_ne!(
        prior_plan.spec().semantics_profile_digest,
        next_plan.spec().semantics_profile_digest
    );

    let runtime = Arc::new(FakeRuntime::new(capabilities(&prior_plan, "tool-runtime")));
    let prior_unused_mcp = Arc::new(FakeRuntime::new(capabilities(&prior_plan, "mcp-runtime")));
    let next_unused_mcp = Arc::new(FakeRuntime::new(capabilities(&next_plan, "mcp-runtime")));
    let (prior_selection, prior_registry) =
        selection(vec![prior_plan.clone()], runtime.clone(), prior_unused_mcp).await;
    let (next_selection, next_registry) =
        selection(vec![next_plan.clone()], runtime.clone(), next_unused_mcp).await;
    let temporary = tempfile::tempdir().unwrap();
    let store = RuntimeBindingStore::new(temporary.path());
    let readiness = Arc::new(RecordingReadiness::default());
    let prior_host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        prior_selection,
        prior_registry,
        store.clone(),
        readiness.clone(),
    );
    let next_host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        next_selection,
        next_registry,
        store.clone(),
        readiness.clone(),
    );

    prior_host
        .prepare_tool(
            &prior_intent,
            tool,
            key(&prior_intent, PluginSurfaceKind::Tool, &tool.id),
        )
        .await
        .unwrap();
    prior_host
        .stop_tool(&prior_intent, tool, "disable-prior-binding")
        .await
        .unwrap();
    next_host
        .prepare_tool(
            &next_intent,
            tool,
            key(&next_intent, PluginSurfaceKind::Tool, &tool.id),
        )
        .await
        .unwrap();

    let receipt = store
        .get_generation(
            &next_intent.scope,
            &next_plan.surface(),
            next_intent.generation,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        receipt.semantics_profile_digest(),
        next_plan
            .spec()
            .semantics_profile_digest
            .as_deref()
            .unwrap()
    );
    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 2);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 2);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn runtime_retirement_resolves_the_exact_provider_from_the_binding_receipt() {
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let intent = intent_generation(&manifest, 29, PluginLifecycleAction::Install);
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
    let readiness = Arc::new(RecordingReadiness::default());
    let activation = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        selection,
        registry.clone(),
        store.clone(),
        readiness.clone(),
    );
    activation
        .prepare_tool(
            &intent,
            tool,
            key(&intent, PluginSurfaceKind::Tool, &tool.id),
        )
        .await
        .unwrap();

    let retirement = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        RuntimeProviderSelection::default(),
        registry,
        store.clone(),
        readiness.clone(),
    );
    retirement
        .remove_tool(&intent, tool, "receipt-owned-retirement")
        .await
        .unwrap();

    assert!(store
        .get_generation(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_none());
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(readiness.removals.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn gateway_drain_failure_preserves_runtime_and_binding_for_replay() {
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
    let readiness = Arc::new(RecordingReadiness {
        fail_drain: true,
        ..RecordingReadiness::default()
    });
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
    let error = host
        .stop_tool(&intent, tool, "drain-must-complete")
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.plugin.gateway_drain_failed");
    assert_eq!(readiness.drains.load(Ordering::SeqCst), 1);
    assert_eq!(runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(runtime.remove_count.load(Ordering::SeqCst), 0);
    assert!(store
        .get_generation(&intent.scope, &plan.surface(), intent.generation)
        .await
        .unwrap()
        .is_some());
}

mod provisioning;

#[derive(Default)]
struct RecordingReadiness {
    calls: AtomicUsize,
    drains: AtomicUsize,
    removals: AtomicUsize,
    fail_tool_binds: AtomicUsize,
    fail_mcp_binds: AtomicUsize,
    fail_drain: bool,
}

#[async_trait]
impl PluginRuntimeServiceReadinessHost for RecordingReadiness {
    async fn bind_tool_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        _plan: &RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
    ) -> UseResult<RuntimeEndpointRef> {
        assert_eq!(runtime_endpoint.port_name, "http");
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_tool_binds
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(UseError::new(
                "use.plugin.gateway_bind_failed",
                "The test Gateway interrupted its Tool route bind.",
            ));
        }
        RuntimeEndpointRef::parse(endpoint_id(intent, &surface.id))
    }

    async fn bind_mcp_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        runtime_endpoint: &RuntimeServiceEndpoint,
        _idempotency_key: &str,
    ) -> UseResult<PluginMcpServiceReadiness> {
        assert_eq!(runtime_endpoint.port_name, "mcp");
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self
            .fail_mcp_binds
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |remaining| {
                remaining.checked_sub(1)
            })
            .is_ok()
        {
            return Err(UseError::new(
                "use.plugin.gateway_bind_failed",
                "The test Gateway interrupted its MCP route bind.",
            ));
        }
        let RuntimeSurfaceContract::McpService {
            protocol_version, ..
        } = plan.contract()
        else {
            panic!("test MCP plan must be a service");
        };
        Ok(PluginMcpServiceReadiness::new(
            RuntimeEndpointRef::parse(endpoint_id(intent, &surface.id))?,
            RuntimeMcpInitializeEvidence::new(
                protocol_version.clone(),
                observation.observed_at_ms + 1,
            )?,
        ))
    }

    async fn drain_service(
        &self,
        _intent: &PluginLifecycleIntent,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
    ) -> UseResult<()> {
        self.drains.fetch_add(1, Ordering::SeqCst);
        if self.fail_drain {
            return Err(UseError::new(
                "use.plugin.gateway_drain_failed",
                "The test Gateway refused to drain its exact route.",
            ));
        }
        Ok(())
    }

    async fn remove_service(
        &self,
        _intent: &PluginLifecycleIntent,
        _receipt: &crate::plugin_runtime::RuntimeServiceBindingReceipt,
        _idempotency_key: &str,
    ) -> UseResult<()> {
        self.removals.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

fn endpoint_id(intent: &PluginLifecycleIntent, surface_id: &str) -> String {
    format!(
        "gateway:{:x}/{surface_id}",
        Sha256::digest(serde_json::to_vec(&intent.scope).unwrap())
    )
}

struct StaticRuntimeFactory {
    provider_id: ProviderId,
    client: Arc<dyn RuntimeClient>,
}

#[async_trait]
impl RuntimeProviderFactory for StaticRuntimeFactory {
    fn provider_id(&self) -> &ProviderId {
        &self.provider_id
    }

    async fn create(&self) -> RuntimeResult<Arc<dyn RuntimeClient>> {
        Ok(self.client.clone())
    }
}

struct FakeRuntime {
    capabilities: RuntimeCapabilities,
    observation: Mutex<Option<RuntimeObservation>>,
    apply_receipts: Mutex<BTreeMap<String, (String, RuntimeObservation)>>,
    apply_count: AtomicUsize,
    stop_count: AtomicUsize,
    remove_count: AtomicUsize,
}

impl FakeRuntime {
    fn new(capabilities: RuntimeCapabilities) -> Self {
        Self {
            capabilities,
            observation: Mutex::new(None),
            apply_receipts: Mutex::new(BTreeMap::new()),
            apply_count: AtomicUsize::new(0),
            stop_count: AtomicUsize::new(0),
            remove_count: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl RuntimeClient for FakeRuntime {
    async fn capabilities(&self) -> RuntimeResult<RuntimeCapabilities> {
        Ok(self.capabilities.clone())
    }

    async fn apply(&self, request: &RuntimeApplyRequest) -> RuntimeResult<RuntimeObservation> {
        let spec_digest = request.spec.digest().map_err(RuntimeError::Protocol)?;
        if let Some((retained_digest, observation)) = self
            .apply_receipts
            .lock()
            .unwrap()
            .get(&request.request_id)
            .cloned()
        {
            if retained_digest != spec_digest {
                return Err(RuntimeError::Protocol(
                    "test Runtime apply request identity was reused for another spec".to_string(),
                ));
            }
            return Ok(observation);
        }
        self.apply_count.fetch_add(1, Ordering::SeqCst);
        let port = request.spec.network.ports.first().ok_or_else(|| {
            RuntimeError::Protocol("test Runtime Service omitted its declared port".to_string())
        })?;
        let mut claims = BTreeMap::new();
        RuntimeServiceEndpoint::node_local_tcp(&port.name, 31_337)
            .map_err(RuntimeError::Protocol)?
            .insert_claim(&mut claims)
            .map_err(RuntimeError::Protocol)?;
        let observation = RuntimeObservation {
            schema: RuntimeObservation::SCHEMA.to_string(),
            unit_id: request.spec.unit_id.clone(),
            generation: request.spec.generation,
            spec_digest: spec_digest.clone(),
            class: request.spec.class,
            state: RuntimeUnitState::Running,
            provider_resource_id: Some("resource-01".to_string()),
            provider_build: Some(self.capabilities.provider_build.clone()),
            observed_at_ms: 1_000,
            started_at_ms: Some(900),
            finished_at_ms: None,
            health: Some(RuntimeHealthObservation {
                state: RuntimeHealthState::Healthy,
                checked_at_ms: 1_000,
                message: None,
            }),
            outputs: Vec::new(),
            usage: None,
            evidence: Some(RuntimeEvidence {
                provider_build: self.capabilities.provider_build.clone(),
                spec_digest: spec_digest.clone(),
                semantics_profile_digest: request.spec.semantics_profile_digest.clone(),
                claims,
            }),
            provider_attestation: None,
            failure: None,
        };
        *self.observation.lock().unwrap() = Some(observation.clone());
        self.apply_receipts.lock().unwrap().insert(
            request.request_id.clone(),
            (spec_digest, observation.clone()),
        );
        Ok(observation)
    }

    async fn inspect(&self, unit_id: &str) -> RuntimeResult<RuntimeInspection> {
        Ok(match self.observation.lock().unwrap().clone() {
            Some(observation) if observation.unit_id == unit_id => RuntimeInspection::Found {
                schema: RuntimeInspection::SCHEMA.to_string(),
                observation: Box::new(observation),
            },
            _ => RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: unit_id.to_string(),
                last_generation: None,
            },
        })
    }

    async fn stop(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeInspection> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        let mut current = self.observation.lock().unwrap();
        let Some(observation) = current.as_mut() else {
            return Ok(RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: request.unit_id.clone(),
                last_generation: None,
            });
        };
        observation.state = RuntimeUnitState::Stopped;
        observation.observed_at_ms = 1_100;
        observation.finished_at_ms = Some(1_100);
        observation.clear_service_endpoints();
        Ok(RuntimeInspection::Found {
            schema: RuntimeInspection::SCHEMA.to_string(),
            observation: Box::new(observation.clone()),
        })
    }

    async fn remove(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeRemoval> {
        self.remove_count.fetch_add(1, Ordering::SeqCst);
        let already_absent = self.observation.lock().unwrap().take().is_none();
        Ok(RuntimeRemoval {
            schema: RuntimeRemoval::SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            unit_id: request.unit_id.clone(),
            generation: request.generation,
            removed_at_ms: 1_200,
            already_absent,
        })
    }

    async fn logs(&self, _query: &RuntimeLogQuery) -> RuntimeResult<Vec<RuntimeLogChunk>> {
        Ok(Vec::new())
    }

    async fn exec(&self, _request: &RuntimeExecRequest) -> RuntimeResult<RuntimeExecResult> {
        Err(RuntimeError::Protocol("unexpected exec".to_string()))
    }
}

async fn selection(
    plans: Vec<RuntimeSurfacePlan>,
    tool: Arc<FakeRuntime>,
    mcp: Arc<FakeRuntime>,
) -> (RuntimeProviderSelection, Arc<RuntimeClientRegistry>) {
    let mut registry = RuntimeClientRegistry::new();
    let providers: [(&str, Arc<dyn RuntimeClient>); 2] =
        [("tool-runtime", tool), ("mcp-runtime", mcp)];
    for (provider, client) in providers {
        registry
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse(provider).unwrap(),
                client,
            }))
            .unwrap();
    }
    let assignments = plans
        .iter()
        .map(|plan| {
            let provider = match plan.context().surface().kind {
                PluginSurfaceKind::Tool => "tool-runtime",
                PluginSurfaceKind::Mcp => "mcp-runtime",
                _ => unreachable!(),
            };
            RuntimeProviderAssignment::new(plan.surface(), provider).unwrap()
        })
        .collect();
    let registry = Arc::new(registry);
    let selection = RuntimeProviderSelector::new(&registry)
        .select(plans, assignments)
        .await
        .unwrap();
    (selection, registry)
}

fn tool_plan(intent: &PluginLifecycleIntent, surface: &ToolSurface) -> RuntimeSurfacePlan {
    let ToolWorkload::Service(service) = &surface.workload else {
        panic!("test Tool must be a service");
    };
    let descriptor = ToolReleaseDescriptor::from_json(TOOL_DESCRIPTOR).unwrap();
    plan_tool_service_release(
        context(intent, PluginSurfaceKind::Tool, &surface.id),
        service,
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap()
}

fn mcp_plan(intent: &PluginLifecycleIntent, surface: &PluginMcpSurface) -> RuntimeSurfacePlan {
    let descriptor = McpReleaseDescriptor::from_json(MCP_DESCRIPTOR).unwrap();
    plan_mcp_service_release(
        context(intent, PluginSurfaceKind::Mcp, &surface.id),
        surface,
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap()
}

fn context(
    intent: &PluginLifecycleIntent,
    kind: PluginSurfaceKind,
    id: &str,
) -> RuntimeSurfaceContext {
    RuntimeSurfaceContext::new(
        intent.package_id.clone(),
        intent.package_digest.clone(),
        intent.scope.clone(),
        intent.plan_digest.clone(),
        PluginSurfaceRef {
            kind,
            id: id.to_string(),
        },
        intent.generation,
    )
    .unwrap()
}

fn artifact(digest: &str, media_type: &str) -> ArtifactRef {
    ArtifactRef {
        uri: format!("oci://registry.example/acme/research@{digest}"),
        digest: digest.to_string(),
        media_type: media_type.to_string(),
    }
}

fn policy() -> RuntimeWorkloadPolicy {
    RuntimeWorkloadPolicy {
        isolation: IsolationLevel::Container,
        resources: RuntimeResourcePolicy {
            cpu_millis: 500,
            memory_bytes: 256 * 1024 * 1024,
            pids: 64,
            ephemeral_storage_bytes: Some(512 * 1024 * 1024),
        },
        mounts: Vec::new(),
        secrets: Vec::new(),
        non_secret_environment: BTreeMap::new(),
        working_directory: None,
    }
}

fn capabilities(plan: &RuntimeSurfacePlan, provider: &str) -> RuntimeCapabilities {
    RuntimeCapabilities {
        schema: RuntimeCapabilities::SCHEMA.to_string(),
        provider_id: ProviderId::parse(provider).unwrap(),
        provider_build: "build-1".to_string(),
        unit_classes: vec![RuntimeUnitClass::Service],
        artifact_media_types: vec![plan.spec().artifact.media_type.clone()],
        isolation_levels: vec![IsolationLevel::Container],
        network_modes: vec![NetworkMode::Service],
        mount_kinds: Vec::<MountKind>::new(),
        health_check_kinds: vec![HealthCheckKind::Http],
        resource_controls: vec![
            ResourceControl::Cpu,
            ResourceControl::Memory,
            ResourceControl::Pids,
            ResourceControl::EphemeralStorage,
        ],
        features: vec![
            RuntimeFeature::DurableIdentity,
            RuntimeFeature::ServiceTcp,
            RuntimeFeature::Stop,
            RuntimeFeature::Remove,
        ],
    }
}

fn package_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3/package")
}

fn intent(manifest: &ExtensionManifest) -> PluginLifecycleIntent {
    intent_generation(manifest, 9, PluginLifecycleAction::Install)
}

fn intent_generation(
    manifest: &ExtensionManifest,
    generation: u64,
    action: PluginLifecycleAction,
) -> PluginLifecycleIntent {
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: format!("runtime-generation-{generation}"),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope: a3s_use_core::PlanScope {
                kind: a3s_use_core::PlanScopeKind::Workspace,
                id: "research".to_string(),
            },
            package_id: manifest.package_id.clone(),
            package_digest: PACKAGE_DIGEST.trim().to_string(),
            manifest_digest: format!("sha256:{:x}", Sha256::digest(MANIFEST.as_bytes())),
            generation,
            action,
            retained_ui_state_surfaces: Vec::new(),
        },
        manifest,
    )
    .unwrap()
}

fn key<'a>(intent: &'a PluginLifecycleIntent, kind: PluginSurfaceKind, id: &str) -> &'a str {
    &intent
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint
                .surface
                .as_ref()
                .is_some_and(|surface| surface.kind == kind && surface.id == id)
        })
        .unwrap()
        .idempotency_key
}
