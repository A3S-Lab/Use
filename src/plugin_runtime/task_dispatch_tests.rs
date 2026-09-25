use std::sync::Arc;

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

use super::test_support::{
    artifact, capabilities, evidence, policy, task_descriptor, FakeRuntime,
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
async fn dispatcher_fails_closed_without_control_installation_authority() {
    let fixture = LegacyPublishFixture::new().await;
    let identity = fixture.install_published_generation(7).await;
    let error = fixture
        .dispatcher()
        .invoke(request(&identity, "invoke-01", "request-01"))
        .await
        .unwrap_err();
    // Legacy extensions/ publication must not authorize invoke; Control is required.
    assert!(
        error.code == "use.control_store.legacy_state_unsupported"
            || error.code == "use.plugin.runtime.generation_unavailable"
            || error.code == "use.plugin.grant_store.control_authority_required"
            || error.code.starts_with("use.control"),
        "unexpected fail-closed code {}",
        error.code
    );
}

#[tokio::test]
async fn dispatcher_fails_closed_when_control_has_no_matching_selection() {
    let fixture = LegacyPublishFixture::new().await;
    let paths = fixture.registry.paths();
    let lifecycle = crate::control_store::ProductionControlLifecycle::from_extension_paths(
        paths,
        crate::control_store::ProductionControlHostDependencies::standalone(
            paths,
            Arc::new(RuntimeClientRegistry::new()),
            None,
        )
        .unwrap(),
    )
    .unwrap();
    lifecycle.initialize().await.unwrap();

    let identity = ExtensionLifecycleIdentity::new(
        "acme/research",
        format!("sha256:{}", "a".repeat(64)),
        format!("sha256:{}", "b".repeat(64)),
        7,
    )
    .unwrap();
    let error = fixture
        .dispatcher()
        .invoke(request(&identity, "invoke-missing", "request-missing"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.runtime.generation_unavailable");
}

/// Pre-Control publication fixture used only to prove invoke no longer accepts
/// legacy `extensions/` / `registry.json` authority.
struct LegacyPublishFixture {
    _temporary: TempDir,
    candidate: ExtensionLifecyclePackage,
    registry: ExtensionRegistry,
    bindings: RuntimeBindingStore,
    providers: Arc<RuntimeClientRegistry>,
    scope: PlanScope,
}

impl LegacyPublishFixture {
    async fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let source = temporary.path().join("package");
        write_release_task_package(&source).await;
        let candidate = ExtensionLifecyclePackage::prepare_local("acme/research", &source, true)
            .await
            .unwrap();
        let scope = PlanScope {
            kind: PlanScopeKind::Workspace,
            id: "workspace-01".to_string(),
        };
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            scope.clone(),
        )
        .unwrap();
        let registry = ExtensionRegistry::new(paths.clone());
        let bindings = RuntimeBindingStore::from_extension_paths(&paths);
        let bootstrap_plan = task_plan(&candidate, &scope, 7);
        let runtime = Arc::new(FakeRuntime::new(capabilities(&bootstrap_plan), true));
        let mut providers = RuntimeClientRegistry::new();
        providers
            .register(Arc::new(StaticRuntimeFactory {
                provider_id: ProviderId::parse("test-runtime").unwrap(),
                client: runtime,
            }))
            .unwrap();
        Self {
            _temporary: temporary,
            candidate,
            registry,
            bindings,
            providers: Arc::new(providers),
            scope,
        }
    }

    async fn install_published_generation(&self, generation: u64) -> ExtensionLifecycleIdentity {
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
        a3s_runtime::contract::NetworkMode::None,
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
