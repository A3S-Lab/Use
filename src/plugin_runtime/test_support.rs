use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;

use a3s_runtime::contract::{
    ArtifactRef, HealthCheckKind, IsolationLevel, MountKind, NetworkMode, ResourceControl,
    RuntimeActionRequest, RuntimeApplyRequest, RuntimeCapabilities, RuntimeExecRequest,
    RuntimeExecResult, RuntimeFeature, RuntimeHealthObservation, RuntimeHealthState,
    RuntimeInspection, RuntimeLogChunk, RuntimeLogQuery, RuntimeLogStream, RuntimeObservation,
    RuntimeRemoval, RuntimeUnitClass, RuntimeUnitState,
};
use a3s_runtime::{ProviderId, RuntimeClient, RuntimeError, RuntimeResult};
use a3s_use_core::{
    McpReleaseDescriptor, PlanEnforcementProfile, PlannedProviderEvidence, PluginSurfaceKind,
    PluginSurfaceRef, ToolReleaseDescriptor,
};
use a3s_use_extension::{
    PluginMcpLaunch, PluginMcpSurface, SurfaceActivation, ToolServiceSurface, ToolTaskSource,
    ToolTaskSurface,
};
use async_trait::async_trait;

use super::*;

pub(super) const DIGEST_A: &str =
    "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const DIGEST_B: &str = "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

pub(super) fn context(kind: PluginSurfaceKind, id: &str) -> RuntimeSurfaceContext {
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

pub(super) fn policy() -> RuntimeWorkloadPolicy {
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

pub(super) fn artifact(digest: &str, media_type: &str) -> ArtifactRef {
    ArtifactRef {
        uri: format!("oci://registry.example/acme/research@{digest}"),
        digest: digest.to_string(),
        media_type: media_type.to_string(),
    }
}

pub(super) fn task_descriptor() -> ToolReleaseDescriptor {
    ToolReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/tool-task-release-v1.json"
    ))
    .unwrap()
}

pub(super) fn task_surface() -> ToolTaskSurface {
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

pub(super) fn service_descriptor() -> ToolReleaseDescriptor {
    ToolReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/tool-service-release-v1.json"
    ))
    .unwrap()
}

pub(super) fn service_surface() -> ToolServiceSurface {
    ToolServiceSurface {
        release: PathBuf::from("releases/service.json"),
        base_path: "/api".to_string(),
        contract: None,
    }
}

pub(super) fn mcp_descriptor() -> McpReleaseDescriptor {
    McpReleaseDescriptor::from_json(include_bytes!(
        "../../crates/core/fixtures/releases/mcp-release-v1.json"
    ))
    .unwrap()
}

pub(super) fn mcp_surface() -> PluginMcpSurface {
    PluginMcpSurface {
        id: "library".to_string(),
        activation: SurfaceActivation::Eager,
        optional: false,
        launch: PluginMcpLaunch::StreamableHttp {
            release: PathBuf::from("releases/mcp.json"),
        },
    }
}

pub(super) fn capabilities(plan: &RuntimeSurfacePlan) -> RuntimeCapabilities {
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

pub(super) fn evidence(
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

pub(super) struct FakeRuntime {
    capabilities: RuntimeCapabilities,
    converge: bool,
    fail_apply: bool,
    pub(super) apply_count: AtomicUsize,
    pub(super) stop_count: AtomicUsize,
    pub(super) remove_count: AtomicUsize,
    logs: Vec<RuntimeLogChunk>,
    observation: Mutex<Option<RuntimeObservation>>,
    removed_generation: AtomicU64,
}

impl FakeRuntime {
    pub(super) fn new(capabilities: RuntimeCapabilities, converge: bool) -> Self {
        Self {
            capabilities,
            converge,
            fail_apply: false,
            apply_count: AtomicUsize::new(0),
            stop_count: AtomicUsize::new(0),
            remove_count: AtomicUsize::new(0),
            logs: Vec::new(),
            observation: Mutex::new(None),
            removed_generation: AtomicU64::new(0),
        }
    }

    pub(super) fn with_logs(mut self, logs: Vec<RuntimeLogChunk>) -> Self {
        self.logs = logs;
        self
    }

    pub(super) fn with_apply_failure(mut self) -> Self {
        self.fail_apply = true;
        self
    }

    pub(super) fn restart_service(&self, started_at_ms: u64, observed_at_ms: u64) {
        let mut observation = self.observation.lock().unwrap();
        let current = observation
            .as_mut()
            .expect("a test Service must be applied before restart");
        current.started_at_ms = Some(started_at_ms);
        current.observed_at_ms = observed_at_ms;
        if let Some(health) = &mut current.health {
            health.checked_at_ms = observed_at_ms;
        }
    }

    pub(super) fn set_service_health_revision(&self, checked_at_ms: u64, observed_at_ms: u64) {
        let mut observation = self.observation.lock().unwrap();
        let current = observation
            .as_mut()
            .expect("a test Service must be applied before changing health");
        current.observed_at_ms = observed_at_ms;
        current
            .health
            .as_mut()
            .expect("a test Service must have health")
            .checked_at_ms = checked_at_ms;
    }
}

#[async_trait]
impl RuntimeClient for FakeRuntime {
    async fn capabilities(&self) -> RuntimeResult<RuntimeCapabilities> {
        Ok(self.capabilities.clone())
    }

    async fn apply(&self, request: &RuntimeApplyRequest) -> RuntimeResult<RuntimeObservation> {
        self.apply_count.fetch_add(1, Ordering::SeqCst);
        if self.fail_apply {
            return Err(RuntimeError::Protocol(
                "injected test apply failure".to_string(),
            ));
        }
        let running = self.converge;
        let task = request.spec.class == RuntimeUnitClass::Task;
        let observation = RuntimeObservation {
            schema: RuntimeObservation::SCHEMA.to_string(),
            unit_id: request.spec.unit_id.clone(),
            generation: request.spec.generation,
            spec_digest: request.spec.digest().map_err(RuntimeError::Protocol)?,
            class: request.spec.class,
            state: if task && running {
                RuntimeUnitState::Succeeded
            } else if running {
                RuntimeUnitState::Running
            } else {
                RuntimeUnitState::Starting
            },
            provider_resource_id: Some("resource-01".to_string()),
            provider_build: Some(self.capabilities.provider_build.clone()),
            observed_at_ms: 1_000,
            started_at_ms: Some(900),
            finished_at_ms: (task && running).then_some(1_000),
            health: (!task)
                .then_some(request.spec.health.as_ref())
                .flatten()
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
        };
        *self.observation.lock().unwrap() = Some(observation.clone());
        Ok(observation)
    }

    async fn inspect(&self, unit_id: &str) -> RuntimeResult<RuntimeInspection> {
        let observation = self.observation.lock().unwrap().clone();
        Ok(match observation {
            Some(observation) if observation.unit_id == unit_id => RuntimeInspection::Found {
                schema: RuntimeInspection::SCHEMA.to_string(),
                observation: Box::new(observation),
            },
            _ => RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: unit_id.to_string(),
                last_generation: match self.removed_generation.load(Ordering::SeqCst) {
                    0 => None,
                    generation => Some(generation),
                },
            },
        })
    }

    async fn stop(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeInspection> {
        self.stop_count.fetch_add(1, Ordering::SeqCst);
        let mut observation = self.observation.lock().unwrap();
        let Some(current) = observation.as_mut() else {
            return Ok(RuntimeInspection::NotFound {
                schema: RuntimeInspection::SCHEMA.to_string(),
                unit_id: request.unit_id.clone(),
                last_generation: None,
            });
        };
        current.state = RuntimeUnitState::Stopped;
        current.observed_at_ms = 1_100;
        current.finished_at_ms = Some(1_100);
        Ok(RuntimeInspection::Found {
            schema: RuntimeInspection::SCHEMA.to_string(),
            observation: Box::new(current.clone()),
        })
    }

    async fn remove(&self, request: &RuntimeActionRequest) -> RuntimeResult<RuntimeRemoval> {
        self.remove_count.fetch_add(1, Ordering::SeqCst);
        let already_absent = self.observation.lock().unwrap().take().is_none();
        self.removed_generation
            .store(request.generation, Ordering::SeqCst);
        Ok(RuntimeRemoval {
            schema: RuntimeRemoval::SCHEMA.to_string(),
            request_id: request.request_id.clone(),
            unit_id: request.unit_id.clone(),
            generation: request.generation,
            removed_at_ms: 1_200,
            already_absent,
        })
    }

    async fn logs(&self, query: &RuntimeLogQuery) -> RuntimeResult<Vec<RuntimeLogChunk>> {
        if query.cursor.is_some() {
            return Ok(Vec::new());
        }
        Ok(self
            .logs
            .iter()
            .filter(|chunk| query.stream.is_none_or(|stream| chunk.stream == stream))
            .cloned()
            .collect())
    }

    async fn exec(&self, _request: &RuntimeExecRequest) -> RuntimeResult<RuntimeExecResult> {
        Err(RuntimeError::Protocol("unexpected exec".to_string()))
    }
}

pub(super) fn log_chunk(
    stream: RuntimeLogStream,
    sequence: u64,
    cursor: &str,
    data: &str,
) -> RuntimeLogChunk {
    RuntimeLogChunk {
        schema: RuntimeLogChunk::SCHEMA.to_string(),
        cursor: cursor.to_string(),
        sequence,
        observed_at_ms: 1_000,
        stream,
        data: data.to_string(),
    }
}
