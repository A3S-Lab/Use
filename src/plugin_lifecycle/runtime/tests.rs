use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use a3s_runtime::contract::{
    ArtifactRef, HealthCheckKind, IsolationLevel, MountKind, NetworkMode, ResourceControl,
    RuntimeActionRequest, RuntimeApplyRequest, RuntimeCapabilities, RuntimeExecRequest,
    RuntimeExecResult, RuntimeFeature, RuntimeHealthObservation, RuntimeHealthState,
    RuntimeInspection, RuntimeLogChunk, RuntimeLogQuery, RuntimeObservation, RuntimeRemoval,
    RuntimeUnitClass, RuntimeUnitState,
};
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeError, RuntimeProviderFactory,
    RuntimeResult,
};
use a3s_use_core::{
    McpReleaseDescriptor, PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef,
    ToolReleaseDescriptor, UseResult,
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
            .get(&intent.scope_id, &surface)
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
    let selection = selection(
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
            .get(&intent.scope_id, &tool_plan.surface())
            .await
            .unwrap(),
        Some(RuntimeBindingReceipt::Service(_))
    ));
    assert!(matches!(
        store
            .get(&intent.scope_id, &mcp_plan.surface())
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
    let removed_mcp = host
        .remove_mcp(&intent, mcp, "uninstall-mcp")
        .await
        .unwrap();
    assert_eq!(tool_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(tool_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(mcp_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert!(store
        .get(&intent.scope_id, &tool_plan.surface())
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(&intent.scope_id, &mcp_plan.surface())
        .await
        .unwrap()
        .is_none());

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
    let prior_selection = selection(
        vec![prior_plan.clone()],
        prior_runtime.clone(),
        prior_unused_mcp,
    )
    .await;
    let next_selection = selection(
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
        store.clone(),
        readiness.clone(),
    );
    let next_host = RuntimePluginSurfaceLifecycleHost::new(
        package_root(),
        next_selection,
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
        .get_generation(&prior_intent.scope_id, &qualified, prior_intent.generation)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get_generation(&next_intent.scope_id, &qualified, next_intent.generation)
        .await
        .unwrap()
        .is_some());
    prior_host
        .remove_tool(&prior_intent, tool, "retire-prior-tool")
        .await
        .unwrap();

    assert!(store
        .get_generation(&prior_intent.scope_id, &qualified, prior_intent.generation)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get_generation(&next_intent.scope_id, &qualified, next_intent.generation)
        .await
        .unwrap()
        .is_some());
    assert_eq!(prior_runtime.stop_count.load(Ordering::SeqCst), 1);
    assert_eq!(prior_runtime.remove_count.load(Ordering::SeqCst), 1);
    assert_eq!(next_runtime.stop_count.load(Ordering::SeqCst), 0);
    assert_eq!(next_runtime.remove_count.load(Ordering::SeqCst), 0);
}

#[derive(Default)]
struct RecordingReadiness {
    calls: AtomicUsize,
}

#[async_trait]
impl PluginRuntimeServiceReadinessHost for RecordingReadiness {
    async fn bind_tool_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &ToolSurface,
        _plan: &RuntimeSurfacePlan,
        _observation: &RuntimeObservation,
        _idempotency_key: &str,
    ) -> UseResult<RuntimeEndpointRef> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        RuntimeEndpointRef::parse(endpoint_id(intent, &surface.id))
    }

    async fn bind_mcp_service(
        &self,
        intent: &PluginLifecycleIntent,
        surface: &PluginMcpSurface,
        plan: &RuntimeSurfacePlan,
        observation: &RuntimeObservation,
        _idempotency_key: &str,
    ) -> UseResult<PluginMcpServiceReadiness> {
        self.calls.fetch_add(1, Ordering::SeqCst);
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
}

fn endpoint_id(intent: &PluginLifecycleIntent, surface_id: &str) -> String {
    format!(
        "gateway:{:x}/{surface_id}",
        Sha256::digest(intent.scope_id.as_bytes())
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
    apply_count: AtomicUsize,
    stop_count: AtomicUsize,
    remove_count: AtomicUsize,
}

impl FakeRuntime {
    fn new(capabilities: RuntimeCapabilities) -> Self {
        Self {
            capabilities,
            observation: Mutex::new(None),
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
        self.apply_count.fetch_add(1, Ordering::SeqCst);
        let observation = RuntimeObservation {
            schema: RuntimeObservation::SCHEMA.to_string(),
            unit_id: request.spec.unit_id.clone(),
            generation: request.spec.generation,
            spec_digest: request.spec.digest().map_err(RuntimeError::Protocol)?,
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
            evidence: None,
            provider_attestation: None,
            failure: None,
        };
        *self.observation.lock().unwrap() = Some(observation.clone());
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
) -> RuntimeProviderSelection {
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
    RuntimeProviderSelector::new(&registry)
        .select(plans, assignments)
        .await
        .unwrap()
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
        intent.scope_id.clone(),
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
            scope_id: "workspace:research".to_string(),
            package_id: manifest.package_id.clone(),
            package_digest: PACKAGE_DIGEST.trim().to_string(),
            manifest_digest: format!("sha256:{:x}", Sha256::digest(MANIFEST.as_bytes())),
            generation,
            action,
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
