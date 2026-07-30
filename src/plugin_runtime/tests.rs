use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use a3s_runtime::contract::{
    ArtifactRef, HealthCheckKind, IsolationLevel, MountKind, NetworkMode, ResourceControl,
    RuntimeActionRequest, RuntimeApplyRequest, RuntimeCapabilities, RuntimeExecRequest,
    RuntimeExecResult, RuntimeFeature, RuntimeHealthObservation, RuntimeHealthState,
    RuntimeInspection, RuntimeLogChunk, RuntimeLogQuery, RuntimeObservation, RuntimeRemoval,
    RuntimeUnitClass, RuntimeUnitState,
};
use a3s_runtime::{ProviderId, RuntimeClient, RuntimeError, RuntimeResult};
use a3s_use_core::{
    McpReleaseDescriptor, PlanEnforcementProfile, PlannedProviderEvidence, PluginSurfaceKind,
    PluginSurfaceRef, ToolReleaseDescriptor, ToolWorkloadContract,
};
use a3s_use_extension::{
    PluginMcpLaunch, PluginMcpSurface, SurfaceActivation, ToolServiceSurface, ToolTaskSource,
    ToolTaskSurface,
};
use async_trait::async_trait;

use super::*;

const DIGEST_A: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn context(kind: PluginSurfaceKind, id: &str) -> RuntimeSurfaceContext {
    RuntimeSurfaceContext::new(
        "acme/research",
        DIGEST_A,
        "workspace-01",
        DIGEST_B,
        PluginSurfaceRef {
            kind,
            id: id.to_string(),
        },
        7,
    )
    .unwrap()
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
        non_secret_environment: BTreeMap::from([(
            "A3S_PLUGIN_MODE".to_string(),
            "managed".to_string(),
        )]),
        working_directory: None,
    }
}

fn artifact(digest: &str, media_type: &str) -> ArtifactRef {
    ArtifactRef {
        uri: format!("oci://registry.example/acme/research@{digest}"),
        digest: digest.to_string(),
        media_type: media_type.to_string(),
    }
}

fn task_descriptor() -> ToolReleaseDescriptor {
    ToolReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/tool-task-release-v1.json"
    ))
    .unwrap()
}

fn task_surface() -> ToolTaskSurface {
    ToolTaskSurface {
        source: ToolTaskSource::Release {
            release: PathBuf::from("releases/task.json"),
        },
        command: "acme-convert".to_string(),
        json_output: true,
        interactive: false,
        timeout_ms: 120_000,
    }
}

fn service_descriptor() -> ToolReleaseDescriptor {
    ToolReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/tool-service-release-v1.json"
    ))
    .unwrap()
}

fn service_surface() -> ToolServiceSurface {
    ToolServiceSurface {
        release: PathBuf::from("releases/service.json"),
        base_path: "/api".to_string(),
        contract: None,
    }
}

fn mcp_descriptor() -> McpReleaseDescriptor {
    McpReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/mcp-release-v1.json"
    ))
    .unwrap()
}

fn mcp_surface() -> PluginMcpSurface {
    PluginMcpSurface {
        id: "library".to_string(),
        activation: SurfaceActivation::Eager,
        optional: false,
        launch: PluginMcpLaunch::StreamableHttp {
            release: PathBuf::from("releases/mcp.json"),
        },
    }
}

#[test]
fn tool_task_plan_binds_invocation_and_release_semantics() {
    let descriptor = task_descriptor();
    let resolved = artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type);
    let invocation =
        RuntimeTaskInvocation::new("invoke-01", vec!["--format".into(), "json".into()]).unwrap();
    let first = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        resolved.clone(),
        invocation,
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let second = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        resolved,
        RuntimeTaskInvocation::new("invoke-02", vec!["--format".into(), "json".into()]).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();

    assert_eq!(first.spec().class, RuntimeUnitClass::Task);
    assert_eq!(
        first.spec().process.command,
        vec!["/usr/local/bin/example-tool"]
    );
    assert_eq!(first.spec().process.args, vec!["--format", "json"]);
    assert_eq!(first.spec().resources.execution_timeout_ms, Some(120_000));
    assert_eq!(first.spec().network.mode, NetworkMode::None);
    assert!(matches!(
        first.contract(),
        RuntimeSurfaceContract::ToolTask {
            command_name,
            json_output: true,
            ..
        } if command_name == "acme-convert"
    ));
    assert_ne!(first.spec().unit_id, second.spec().unit_id);
    assert_eq!(
        first.spec().semantics_profile_digest,
        second.spec().semantics_profile_digest
    );
    assert!(first
        .spec()
        .semantics_profile_digest
        .as_deref()
        .unwrap()
        .starts_with("sha256:"));
    assert!(first.spec().validate().is_ok());
}

#[test]
fn task_plan_rejects_unrepresentable_exit_code_semantics() {
    let mut descriptor = task_descriptor();
    let ToolWorkloadContract::Task {
        success_exit_codes, ..
    } = &mut descriptor.workload
    else {
        panic!("fixture should be a Task");
    };
    *success_exit_codes = vec![0, 2];
    let error = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.task_semantics_unsupported");
}

#[test]
fn service_plans_preserve_native_http_and_mcp_contracts() {
    let tool = service_descriptor();
    let tool_plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &tool,
        artifact(&tool.artifact.digest, &tool.artifact.media_type),
        policy(),
    )
    .unwrap();
    assert_eq!(tool_plan.spec().class, RuntimeUnitClass::Service);
    assert_eq!(tool_plan.spec().network.mode, NetworkMode::Service);
    assert_eq!(tool_plan.spec().network.ports[0].container_port, 8080);
    assert!(tool_plan.spec().process.command.is_empty());
    assert!(matches!(
        tool_plan.contract(),
        RuntimeSurfaceContract::ToolService { base_path, .. } if base_path == "/api"
    ));

    let mcp = mcp_descriptor();
    let mcp_plan = plan_mcp_service_release(
        context(PluginSurfaceKind::Mcp, "library"),
        &mcp_surface(),
        &mcp,
        artifact(&mcp.artifact.digest, &mcp.artifact.media_type),
        policy(),
    )
    .unwrap();
    assert_eq!(mcp_plan.spec().network.ports[0].container_port, 8080);
    assert!(matches!(
        mcp_plan.contract(),
        RuntimeSurfaceContract::McpService {
            endpoint_path,
            protocol_version,
            ..
        } if endpoint_path == "/mcp" && protocol_version == "2025-06-18"
    ));
}

#[test]
fn release_plan_rejects_artifact_substitution() {
    let descriptor = service_descriptor();
    let error = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(DIGEST_A, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.artifact_mismatch");
}

#[tokio::test]
async fn explicit_provider_evidence_is_rechecked_without_fallback() {
    let descriptor = task_descriptor();
    let plan = plan_tool_task_release(
        context(PluginSurfaceKind::Tool, "convert"),
        &task_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("invoke-01", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities.clone(), true));
    let client = PluginRuntimeClient::new(runtime);
    let binding = client.prepare_task(&plan, &provider).await.unwrap();
    assert_eq!(binding.provider_id, "test-runtime");
    assert_eq!(binding.artifact_digest, plan.spec().artifact.digest);

    let mut changed = capabilities;
    changed.provider_build = "build-2".to_string();
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(changed, true)));
    let error = client.prepare_task(&plan, &provider).await.unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.provider_evidence_changed");
}

#[tokio::test]
async fn healthy_service_activation_requires_an_opaque_endpoint_binding() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let runtime = Arc::new(FakeRuntime::new(capabilities, true));
    let client = PluginRuntimeClient::new(runtime.clone());
    let activation = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap();
    let receipt = activation
        .into_tool_service_receipt(RuntimeEndpointRef::parse("gateway:workspace-01/index").unwrap())
        .unwrap();

    assert_eq!(runtime.apply_count.load(Ordering::SeqCst), 1);
    assert_eq!(receipt.schema, RUNTIME_SERVICE_BINDING_SCHEMA);
    assert_eq!(receipt.endpoint_ref.as_str(), "gateway:workspace-01/index");
    assert_eq!(receipt.provider_build_id, "build-1");
    assert!(RuntimeEndpointRef::parse("https://user:token@example.com").is_err());
    assert!(!serde_json::to_string(&receipt).unwrap().contains("token"));
}

#[tokio::test]
async fn service_binding_is_not_published_before_runtime_convergence() {
    let descriptor = service_descriptor();
    let plan = plan_tool_service_release(
        context(PluginSurfaceKind::Tool, "index"),
        &service_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(capabilities, false)));

    let error = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.not_converged");
}

#[tokio::test]
async fn mcp_service_binding_requires_matching_initialize_evidence() {
    let descriptor = mcp_descriptor();
    let plan = plan_mcp_service_release(
        context(PluginSurfaceKind::Mcp, "library"),
        &mcp_surface(),
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        policy(),
    )
    .unwrap();
    let capabilities = capabilities(&plan);
    let provider = evidence(&plan, &capabilities);
    let client = PluginRuntimeClient::new(Arc::new(FakeRuntime::new(capabilities, true)));
    let activation = client
        .apply_service(&plan, &provider, "operation-01", Some(9_999_999))
        .await
        .unwrap();
    let endpoint = RuntimeEndpointRef::parse("gateway:workspace-01/library").unwrap();

    assert!(activation
        .clone()
        .into_tool_service_receipt(endpoint.clone())
        .is_err());
    let wrong_protocol = RuntimeMcpInitializeEvidence::new("2024-11-05", 1_001).unwrap();
    assert!(activation
        .clone()
        .into_mcp_service_receipt(endpoint.clone(), wrong_protocol)
        .is_err());
    let initialize = RuntimeMcpInitializeEvidence::new("2025-06-18", 1_001).unwrap();
    let receipt = activation
        .into_mcp_service_receipt(endpoint, initialize)
        .unwrap();
    assert!(matches!(
        receipt.readiness,
        RuntimeServiceReadinessEvidence::McpInitialized { .. }
    ));
}

fn capabilities(plan: &RuntimeSurfacePlan) -> RuntimeCapabilities {
    RuntimeCapabilities {
        schema: RuntimeCapabilities::SCHEMA.to_string(),
        provider_id: ProviderId::parse("test-runtime").unwrap(),
        provider_build: "build-1".to_string(),
        unit_classes: vec![RuntimeUnitClass::Task, RuntimeUnitClass::Service],
        artifact_media_types: vec![plan.spec().artifact.media_type.clone()],
        isolation_levels: vec![IsolationLevel::Container],
        network_modes: vec![NetworkMode::None, NetworkMode::Service],
        mount_kinds: Vec::<MountKind>::new(),
        health_check_kinds: vec![HealthCheckKind::Http],
        resource_controls: vec![
            ResourceControl::Cpu,
            ResourceControl::Memory,
            ResourceControl::Pids,
            ResourceControl::EphemeralStorage,
            ResourceControl::ExecutionTimeout,
        ],
        features: vec![
            RuntimeFeature::DurableIdentity,
            RuntimeFeature::Logs,
            RuntimeFeature::Stop,
            RuntimeFeature::Remove,
        ],
    }
}

fn evidence(
    plan: &RuntimeSurfacePlan,
    capabilities: &RuntimeCapabilities,
) -> PlannedProviderEvidence {
    PlannedProviderEvidence {
        surface: plan.surface(),
        provider_id: capabilities.provider_id.to_string(),
        provider_build_id: capabilities.provider_build.clone(),
        capability_digest: runtime_capabilities_digest(capabilities).unwrap(),
        semantics_profile_digest: plan.spec().semantics_profile_digest.clone().unwrap(),
        enforcement: PlanEnforcementProfile::Container,
    }
}

struct FakeRuntime {
    capabilities: RuntimeCapabilities,
    converge: bool,
    apply_count: AtomicUsize,
}

impl FakeRuntime {
    fn new(capabilities: RuntimeCapabilities, converge: bool) -> Self {
        Self {
            capabilities,
            converge,
            apply_count: AtomicUsize::new(0),
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
        let running = self.converge;
        Ok(RuntimeObservation {
            schema: RuntimeObservation::SCHEMA.to_string(),
            unit_id: request.spec.unit_id.clone(),
            generation: request.spec.generation,
            spec_digest: request.spec.digest().map_err(RuntimeError::Protocol)?,
            class: request.spec.class,
            state: if running {
                RuntimeUnitState::Running
            } else {
                RuntimeUnitState::Starting
            },
            provider_resource_id: Some("resource-01".to_string()),
            provider_build: Some(self.capabilities.provider_build.clone()),
            observed_at_ms: 1_000,
            started_at_ms: Some(900),
            finished_at_ms: None,
            health: request
                .spec
                .health
                .as_ref()
                .map(|_| RuntimeHealthObservation {
                    state: if running {
                        RuntimeHealthState::Healthy
                    } else {
                        RuntimeHealthState::Starting
                    },
                    checked_at_ms: 1_000,
                    message: None,
                }),
            outputs: Vec::new(),
            usage: None,
            evidence: None,
            provider_attestation: None,
            failure: None,
        })
    }

    async fn inspect(&self, _unit_id: &str) -> RuntimeResult<RuntimeInspection> {
        Err(RuntimeError::Protocol("unexpected inspect".to_string()))
    }

    async fn stop(&self, _request: &RuntimeActionRequest) -> RuntimeResult<RuntimeInspection> {
        Err(RuntimeError::Protocol("unexpected stop".to_string()))
    }

    async fn remove(&self, _request: &RuntimeActionRequest) -> RuntimeResult<RuntimeRemoval> {
        Err(RuntimeError::Protocol("unexpected remove".to_string()))
    }

    async fn logs(&self, _query: &RuntimeLogQuery) -> RuntimeResult<Vec<RuntimeLogChunk>> {
        Err(RuntimeError::Protocol("unexpected logs".to_string()))
    }

    async fn exec(&self, _request: &RuntimeExecRequest) -> RuntimeResult<RuntimeExecResult> {
        Err(RuntimeError::Protocol("unexpected exec".to_string()))
    }
}
