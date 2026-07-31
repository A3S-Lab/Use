use std::path::Path;
use std::sync::Arc;

use a3s_runtime::contract::NetworkMode;
use a3s_runtime::{
    ProviderId, RuntimeClient, RuntimeClientRegistry, RuntimeProviderFactory, RuntimeResult,
};
use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind, PluginSurfaceRef};
use a3s_use_extension::{
    inspect_release_bundle, ExtensionManifest, ExtensionPaths, ExtensionReceipt, ExtensionRegistry,
    ExtensionTrust, InstalledExtension, ToolWorkload,
};
use async_trait::async_trait;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

use crate::plugin_runtime::test_support::{
    artifact, capabilities, evidence, policy, task_descriptor, FakeRuntime,
};
use crate::plugin_runtime::{
    plan_tool_task_release, PluginRuntimeClient, RuntimeBindingReceipt, RuntimeBindingStore,
    RuntimeSurfaceContext, RuntimeTaskInvocation,
};
use crate::surface_reconciler::{PluginObservedState, SurfaceObservedState, SurfaceOwner};

use super::*;

const SESSION_PLUGIN: &str = r#"
extension "acme/session" {
  schema_version = 3
  version        = "1.0.0"
  route          = "session"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  repository {
    url      = "https://github.com/acme/session"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload    = "task"
    interface   = "cli"
    release     = "releases/task.json"
    command     = "acme-session-convert"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = "lazy"
    optional    = false
  }

  mcp "local" {
    transport  = "stdio"
    executable = "bin/session-mcp"
    args       = ["serve", "--stdio"]
    activation = "lazy"
    optional   = false
  }

  skill "guide" {
    path          = "skills/guide/SKILL.md"
    requires_tool = ["convert"]
    requires_mcp  = ["local"]
    optional      = false
  }

  ui "panel" {
    entry     = "ui/panel/index.html"
    styles    = []
    scripts   = []
    skill     = "guide"
    bind_tool = ["convert"]
    bind_mcp  = ["local"]
    optional  = false
  }
}
"#;

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

struct SessionFixture {
    _root: TempDir,
    registry: ExtensionRegistry,
    store: RuntimeBindingStore,
    providers: RuntimeClientRegistry,
    extension: InstalledExtension,
    package_digest: String,
}

#[tokio::test]
async fn scoped_snapshot_composes_runtime_stdio_skill_and_ui_observations() {
    let fixture = session_fixture("workspace-a").await;
    let observations = host_observations("workspace-a", &fixture.package_digest);
    let builder = session_builder(&fixture);

    let snapshot = builder.snapshot(&observations).await.unwrap();
    let binding = snapshot
        .capabilities
        .iter()
        .find(|binding| binding.id == fixture.extension.receipt.component_id)
        .unwrap();
    let reconciliation = binding.reconciliation.as_ref().unwrap();

    assert_eq!(snapshot.scope_id, "workspace-a");
    assert_eq!(snapshot.revision.len(), 64);
    assert!(binding.enabled);
    assert_eq!(binding.readiness, Readiness::Ready);
    assert_eq!(reconciliation.observed, PluginObservedState::Ready);
    assert!(reconciliation.capability_ready);
    assert_eq!(
        surface_owner(reconciliation, PluginSurfaceKind::Tool, "convert"),
        SurfaceOwner::Runtime
    );
    assert_eq!(
        surface_owner(reconciliation, PluginSurfaceKind::Mcp, "local"),
        SurfaceOwner::McpHost
    );
    assert_eq!(
        surface_owner(reconciliation, PluginSurfaceKind::Skill, "guide"),
        SurfaceOwner::SkillHost
    );
    assert_eq!(
        surface_owner(reconciliation, PluginSurfaceKind::Ui, "panel"),
        SurfaceOwner::UiHost
    );
    assert_eq!(
        surface_state(reconciliation, PluginSurfaceKind::Tool, "convert"),
        SurfaceObservedState::Prepared
    );
    assert!(reconciliation.publishes(PluginSurfaceKind::Skill, "guide"));
    assert_eq!(binding.skills.len(), 1);

    let repeated = builder.snapshot(&observations).await.unwrap();
    assert_eq!(snapshot, repeated);
}

#[tokio::test]
async fn scoped_snapshot_does_not_reuse_runtime_bindings_from_another_scope() {
    let fixture = session_fixture("workspace-a").await;
    let in_scope = host_observations("workspace-a", &fixture.package_digest);
    let other_scope = host_observations("workspace-b", &fixture.package_digest);
    let builder = session_builder(&fixture);

    let ready = builder.snapshot(&in_scope).await.unwrap();
    let isolated = builder.snapshot(&other_scope).await.unwrap();
    let binding = isolated
        .capabilities
        .iter()
        .find(|binding| binding.id == fixture.extension.receipt.component_id)
        .unwrap();
    let reconciliation = binding.reconciliation.as_ref().unwrap();

    assert_ne!(ready.revision, isolated.revision);
    assert_eq!(isolated.scope_id, "workspace-b");
    assert!(!binding.enabled);
    assert_eq!(reconciliation.observed, PluginObservedState::Reconciling);
    assert_eq!(
        surface_state(reconciliation, PluginSurfaceKind::Tool, "convert"),
        SurfaceObservedState::Pending
    );
}

#[tokio::test]
async fn session_revision_binds_the_exact_runtime_binding_generation() {
    let fixture = session_fixture("workspace-a").await;
    let observations = host_observations("workspace-a", &fixture.package_digest);
    let builder = session_builder(&fixture);
    let first = builder.snapshot(&observations).await.unwrap();
    let qualified = PlanQualifiedSurfaceRef {
        package_id: "acme/session".to_string(),
        surface: surface(PluginSurfaceKind::Tool, "convert"),
    };
    let receipt = fixture
        .store
        .get("workspace-a", &qualified)
        .await
        .unwrap()
        .unwrap();
    let RuntimeBindingReceipt::Task(mut task) = receipt else {
        panic!("the session fixture must contain a Runtime Task receipt");
    };
    task.generation = 2;
    fixture
        .store
        .put(&RuntimeBindingReceipt::Task(task))
        .await
        .unwrap();

    let second = builder.snapshot(&observations).await.unwrap();

    assert_eq!(first.generation, second.generation);
    assert_ne!(first.revision, second.revision);
    assert_eq!(
        second.runtime_observations[0].surfaces()[0].generation(),
        Some(2)
    );
}

#[tokio::test]
async fn host_observations_cannot_claim_a_runtime_owned_surface() {
    let fixture = session_fixture("workspace-a").await;
    let mut observations = host_observation_entries(&fixture.package_digest);
    observations.push(
        CapabilityHostSurfaceObservation::new(
            "acme/session",
            &fixture.package_digest,
            surface(PluginSurfaceKind::Tool, "convert"),
            CapabilityHostSurfaceOwner::ToolHost,
            CapabilitySurfaceObservedState::Prepared,
        )
        .unwrap(),
    );
    let observations = CapabilitySessionObservations::new("workspace-a", observations).unwrap();
    let builder = session_builder(&fixture);

    let error = builder.snapshot(&observations).await.unwrap_err();
    assert_eq!(error.code, "use.capability.session_observation_invalid");
}

#[test]
fn scoped_observation_contract_rejects_duplicates_and_is_send_sync() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let observation = CapabilityHostSurfaceObservation::new(
        "acme/session",
        &digest,
        surface(PluginSurfaceKind::Skill, "guide"),
        CapabilityHostSurfaceOwner::SkillHost,
        CapabilitySurfaceObservedState::Prepared,
    )
    .unwrap();
    let error =
        CapabilitySessionObservations::new("workspace-a", vec![observation.clone(), observation])
            .unwrap_err();
    assert_eq!(error.code, "use.capability.session_observation_invalid");

    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilitySessionObservations>();
    assert_send_sync::<CapabilitySessionSnapshot>();
    assert_send_sync::<CapabilitySessionSnapshotBuilder<'static>>();
}

async fn session_fixture(scope_id: &str) -> SessionFixture {
    let root = TempDir::new().unwrap();
    let paths = ExtensionPaths::new(root.path().join("data"), root.path().join("state"));
    let package_root = paths
        .data_root()
        .join("extensions/acme/session/1.0.0-fixture");
    write_file(
        &package_root.join("a3s-use-extension.acl"),
        SESSION_PLUGIN.as_bytes(),
    )
    .await;
    write_file(
        &package_root.join("releases/task.json"),
        include_bytes!("../crates/core/fixtures/releases/tool-task-release-v1.json"),
    )
    .await;
    write_file(&package_root.join("bin/session-mcp"), b"fixture executable").await;
    write_file(
        &package_root.join("skills/guide/SKILL.md"),
        b"# Session guide\n",
    )
    .await;
    write_file(
        &package_root.join("ui/panel/index.html"),
        b"<main>Session</main>",
    )
    .await;

    let package = inspect_release_bundle(&package_root).await.unwrap();
    let manifest = ExtensionManifest::parse_acl(SESSION_PLUGIN).unwrap();
    let receipt = ExtensionReceipt {
        schema_version: 1,
        package_id: manifest.package_id.clone(),
        component_id: format!("use/{}", manifest.package_id),
        route: manifest.route.clone(),
        version: manifest.version.clone(),
        package_root: package_root.clone(),
        manifest_sha256: format!("{:x}", Sha256::digest(SESSION_PLUGIN.as_bytes())),
        package_sha256: Some(package.package_sha256),
        trust: ExtensionTrust::LocalExplicit,
        registry: None,
        verified_catalog: None,
        installed_at_unix: 1,
        enabled: true,
    };
    write_file(
        &paths.state_root().join("extensions/acme/session.json"),
        &serde_json::to_vec(&receipt).unwrap(),
    )
    .await;
    let registry = ExtensionRegistry::new(paths.clone());
    registry.snapshot().await.unwrap();
    let extension = registry.get("acme/session").await.unwrap().unwrap();
    let package_digest = format!(
        "sha256:{}",
        extension.receipt.package_sha256.as_deref().unwrap()
    );
    let store = RuntimeBindingStore::from_extension_paths(&paths);
    let mut providers = RuntimeClientRegistry::new();
    install_runtime_task(
        scope_id,
        &package_digest,
        &extension,
        &store,
        &mut providers,
    )
    .await;

    SessionFixture {
        _root: root,
        registry,
        store,
        providers,
        extension,
        package_digest,
    }
}

async fn install_runtime_task(
    scope_id: &str,
    package_digest: &str,
    extension: &InstalledExtension,
    store: &RuntimeBindingStore,
    providers: &mut RuntimeClientRegistry,
) {
    let task = extension
        .manifest
        .tools
        .iter()
        .find(|tool| tool.id == "convert")
        .and_then(|tool| match &tool.workload {
            ToolWorkload::Task(task) => Some(task),
            ToolWorkload::Service(_) => None,
        })
        .unwrap();
    let descriptor = task_descriptor();
    let context = RuntimeSurfaceContext::new(
        "acme/session",
        package_digest,
        scope_id,
        format!("sha256:{}", "b".repeat(64)),
        surface(PluginSurfaceKind::Tool, "convert"),
        1,
    )
    .unwrap();
    let plan = plan_tool_task_release(
        context,
        task,
        &descriptor,
        artifact(&descriptor.artifact.digest, &descriptor.artifact.media_type),
        RuntimeTaskInvocation::new("session-template", Vec::new()).unwrap(),
        policy(),
        NetworkMode::None,
    )
    .unwrap();
    let runtime_capabilities = capabilities(&plan);
    let provider_evidence = evidence(&plan, &runtime_capabilities);
    let runtime = Arc::new(FakeRuntime::new(runtime_capabilities.clone(), true));
    let prepared = PluginRuntimeClient::new(runtime.clone())
        .prepare_task(&plan, &provider_evidence)
        .await
        .unwrap();
    store
        .put(&RuntimeBindingReceipt::Task(prepared))
        .await
        .unwrap();
    providers
        .register(Arc::new(StaticRuntimeFactory {
            provider_id: runtime_capabilities.provider_id,
            client: runtime,
        }))
        .unwrap();
}

fn host_observations(scope_id: &str, digest: &str) -> CapabilitySessionObservations {
    CapabilitySessionObservations::new(scope_id, host_observation_entries(digest)).unwrap()
}

fn session_builder(fixture: &SessionFixture) -> CapabilitySessionSnapshotBuilder<'_> {
    CapabilitySessionSnapshotBuilder::new_for_host_version(
        &fixture.registry,
        &fixture.store,
        &fixture.providers,
        "0.3.0",
    )
}

fn host_observation_entries(digest: &str) -> Vec<CapabilityHostSurfaceObservation> {
    [
        (
            PluginSurfaceKind::Mcp,
            "local",
            CapabilityHostSurfaceOwner::McpHost,
        ),
        (
            PluginSurfaceKind::Skill,
            "guide",
            CapabilityHostSurfaceOwner::SkillHost,
        ),
        (
            PluginSurfaceKind::Ui,
            "panel",
            CapabilityHostSurfaceOwner::UiHost,
        ),
    ]
    .into_iter()
    .map(|(kind, id, owner)| {
        CapabilityHostSurfaceObservation::new(
            "acme/session",
            digest,
            surface(kind, id),
            owner,
            CapabilitySurfaceObservedState::Prepared,
        )
        .unwrap()
    })
    .collect()
}

async fn write_file(path: &Path, bytes: &[u8]) {
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, bytes).await.unwrap();
}

fn surface(kind: PluginSurfaceKind, id: &str) -> PluginSurfaceRef {
    PluginSurfaceRef {
        kind,
        id: id.to_string(),
    }
}

fn surface_owner(
    snapshot: &SurfaceReconcileSnapshot,
    kind: PluginSurfaceKind,
    id: &str,
) -> SurfaceOwner {
    snapshot
        .surfaces
        .iter()
        .find(|surface| surface.surface == self::surface(kind, id))
        .map(|surface| surface.owner)
        .unwrap()
}

fn surface_state(
    snapshot: &SurfaceReconcileSnapshot,
    kind: PluginSurfaceKind,
    id: &str,
) -> SurfaceObservedState {
    snapshot
        .surfaces
        .iter()
        .find(|surface| surface.surface == self::surface(kind, id))
        .map(|surface| surface.observed)
        .unwrap()
}
