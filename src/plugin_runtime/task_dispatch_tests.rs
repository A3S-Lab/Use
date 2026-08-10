use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use a3s_runtime::contract::{NetworkMode, RuntimeLogStream};
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{PlanScope, PlanScopeKind, PluginSurfaceKind, PluginSurfaceRef};
use a3s_use_extension::{
    ExtensionLifecycleIdentity, ExtensionLifecyclePackage, ExtensionPaths, ExtensionRegistry,
    ToolWorkload,
};
use async_trait::async_trait;
use tempfile::TempDir;
use tokio::fs;
use tokio::sync::Notify;

use super::test_support::{
    artifact, capabilities, evidence, log_chunk, policy, task_descriptor, FakeRuntime,
};
use super::*;

const GRANT_DIGEST: &str =
    "sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

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

#[test]
fn dispatcher_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RuntimeTaskDispatcher>();
    assert_send_sync::<RuntimeTaskDispatchRequest>();
}

#[tokio::test]
async fn dispatcher_survives_restart_and_rejects_a_stale_upgrade_generation() {
    let fixture = DispatchFixture::new(None).await;
    let first = fixture.install_generation(7).await;
    let dispatcher = fixture.dispatcher();

    let initial = dispatcher
        .invoke(request(&first, "invoke-01", "request-01"))
        .await
        .unwrap();
    assert_eq!(initial.stdout, "{\"ok\":true}\n");

    // A new dispatcher has only durable Registry/binding state plus the host's
    // configured provider registry. No operation-plan record is retained.
    let restarted = fixture.dispatcher();
    restarted
        .invoke(request(&first, "invoke-02", "request-02"))
        .await
        .unwrap();

    let next = fixture.install_generation(8).await;
    let stale = restarted
        .invoke(request(&first, "invoke-stale", "request-stale"))
        .await
        .unwrap_err();
    assert_eq!(stale.code, "use.plugin.runtime.generation_unavailable");

    restarted
        .invoke(request(&next, "invoke-03", "request-03"))
        .await
        .unwrap();
    assert_eq!(fixture.runtime.apply_count.load(Ordering::SeqCst), 3);
    assert_eq!(fixture.runtime.remove_count.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn dispatcher_lease_blocks_retirement_and_hide_rejects_new_calls() {
    let started = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let fixture = DispatchFixture::new(Some((started.clone(), release.clone()))).await;
    let identity = fixture.install_generation(7).await;
    let dispatcher = fixture.dispatcher();
    let active_request = request(&identity, "invoke-active", "request-active");
    let active = tokio::spawn(async move { dispatcher.invoke(active_request).await });

    tokio::time::timeout(Duration::from_secs(1), started.notified())
        .await
        .unwrap();
    fixture
        .registry
        .hide_lifecycle_package(&identity)
        .await
        .unwrap();
    let drain = fixture
        .registry
        .drain_lifecycle_package(&identity, Duration::from_millis(10))
        .await
        .unwrap_err();
    assert_eq!(drain.code, "use.extension.drain_timeout");

    let rejected = fixture
        .dispatcher()
        .invoke(request(&identity, "invoke-late", "request-late"))
        .await
        .unwrap_err();
    assert_eq!(rejected.code, "use.plugin.runtime.generation_unavailable");
    assert_eq!(fixture.runtime.apply_count.load(Ordering::SeqCst), 1);

    release.notify_one();
    active.await.unwrap().unwrap();
    fixture
        .registry
        .drain_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
}

struct DispatchFixture {
    _temporary: TempDir,
    candidate: ExtensionLifecyclePackage,
    registry: ExtensionRegistry,
    bindings: RuntimeBindingStore,
    providers: Arc<RuntimeClientRegistry>,
    runtime: Arc<FakeRuntime>,
    scope: PlanScope,
}

impl DispatchFixture {
    async fn new(apply_gate: Option<(Arc<Notify>, Arc<Notify>)>) -> Self {
        let temporary = TempDir::new().unwrap();
        let source = temporary.path().join("package");
        write_release_task_package(&source).await;
        let candidate = ExtensionLifecyclePackage::prepare_local("acme/research", &source, true)
            .await
            .unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
        );
        let registry = ExtensionRegistry::new(paths.clone());
        let bindings = RuntimeBindingStore::from_extension_paths(&paths);
        let scope = PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "workspace-01".to_string(),
        };
        let bootstrap_plan = task_plan(&candidate, &scope, 7);
        let runtime_capabilities = capabilities(&bootstrap_plan);
        let mut runtime = FakeRuntime::new(runtime_capabilities, true).with_logs(vec![log_chunk(
            RuntimeLogStream::Stdout,
            1,
            "stdout-1",
            "{\"ok\":true}\n",
        )]);
        if let Some((started, release)) = apply_gate {
            runtime = runtime.with_apply_gate(started, release);
        }
        let runtime = Arc::new(runtime);
        let mut providers = RuntimeClientRegistry::new();
        providers
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse("test-runtime").unwrap(),
                client: runtime.clone(),
            }))
            .unwrap();
        Self {
            _temporary: temporary,
            candidate,
            registry,
            bindings,
            providers: Arc::new(providers),
            runtime,
            scope,
        }
    }

    async fn install_generation(&self, generation: u64) -> ExtensionLifecycleIdentity {
        let identity = ExtensionLifecycleIdentity::new(
            self.candidate.package_id(),
            self.candidate.package_digest(),
            self.candidate.manifest_digest(),
            generation,
        )
        .unwrap();
        self.registry
            .commit_lifecycle_package(&identity, &self.candidate)
            .await
            .unwrap();
        let plan = task_plan(&self.candidate, &self.scope, generation);
        let provider = evidence(&plan, &capabilities(&plan));
        let binding = RuntimePreparedTaskBinding::from_plan(&plan, &provider).unwrap();
        self.bindings
            .put(&RuntimeBindingReceipt::Task(binding))
            .await
            .unwrap();
        self.registry
            .publish_lifecycle_package(&identity)
            .await
            .unwrap();
        identity
    }

    fn dispatcher(&self) -> RuntimeTaskDispatcher {
        RuntimeTaskDispatcher::new(
            self.registry.clone(),
            self.bindings.clone(),
            self.providers.clone(),
        )
    }
}

fn task_plan(
    candidate: &ExtensionLifecyclePackage,
    scope: &PlanScope,
    generation: u64,
) -> RuntimeSurfacePlan {
    let tool = candidate
        .manifest()
        .tools
        .iter()
        .find(|surface| surface.id == "convert")
        .unwrap();
    let ToolWorkload::Task(surface) = &tool.workload else {
        panic!("fixture Tool must be a Task");
    };
    let descriptor = task_descriptor();
    plan_tool_task_release(
        RuntimeSurfaceContext::new(
            candidate.package_id(),
            candidate.package_digest(),
            scope.clone(),
            GRANT_DIGEST,
            PluginSurfaceRef {
                kind: PluginSurfaceKind::Tool,
                id: "convert".to_string(),
            },
            generation,
        )
        .unwrap(),
        surface,
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("planning-template", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap()
}

fn request(
    identity: &ExtensionLifecycleIdentity,
    invocation_id: &str,
    request_id: &str,
) -> RuntimeTaskDispatchRequest {
    RuntimeTaskDispatchRequest::new(
        identity.clone(),
        PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "workspace-01".to_string(),
        },
        "convert",
        RuntimeTaskInvocation::new(invocation_id, vec!["--format".into(), "json".into()]).unwrap(),
        request_id,
        Some(9_999_999),
    )
    .unwrap()
}

async fn write_release_task_package(root: &std::path::Path) {
    fs::create_dir_all(root.join("releases")).await.unwrap();
    fs::write(root.join("README.md"), "# Managed Runtime Task fixture\n")
        .await
        .unwrap();
    fs::write(
        root.join("releases/task.json"),
        include_bytes!("../../crates/core/fixtures/releases/tool-task-release-v1.json"),
    )
    .await
    .unwrap();
    fs::write(
        root.join("a3s-use-extension.acl"),
        r#"extension "acme/research" {
  schema_version = 3
  version        = "2.0.0"
  route          = "research"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["execute"]

  repository {
    url      = "https://github.com/acme/research"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    release     = "releases/task.json"
    command     = "acme-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }
}
"#,
    )
    .await
    .unwrap();
}
