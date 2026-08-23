use std::path::{Path, PathBuf};

use a3s_use_core::{PlanQualifiedSurfaceRef, PluginSurfaceKind};
use a3s_use_extension::ExtensionManifest;
use sha2::{Digest, Sha256};

use crate::plugin_lifecycle::{
    PluginFlowLifecycleHost, PluginLifecycleAction, PluginLifecycleIntent,
    PluginLifecycleIntentSpec,
};

use super::{A3sFlowLifecycleHost, FlowRuntimeBindingStore};

const MANIFEST: &str = r#"
extension "acme/review" {
  schema_version = 3
  version        = "1.0.0"
  route          = "review"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {
    url      = "https://github.com/acme/review"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  flow "review" {
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/review.ts"
    export        = "run"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = []
    optional      = false
  }
}
"#;

#[tokio::test]
async fn a3s_flow_preflight_is_retained_per_exact_package_generation() {
    let temporary = tempfile::tempdir().unwrap();
    let package_root = temporary.path().join("package");
    std::fs::create_dir_all(package_root.join("flows")).unwrap();
    std::fs::write(
        package_root.join("flows/review.ts"),
        "export function run() { return { type: 'complete', output: {} }; }\n",
    )
    .unwrap();
    let compiler = fake_flow_compiler(temporary.path());

    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let store = FlowRuntimeBindingStore::new(temporary.path().join("state"));
    let host = A3sFlowLifecycleHost::new(
        &package_root,
        &compiler,
        temporary.path().join("cache"),
        store.clone(),
    );
    let first = intent(&manifest, 7);
    let first_key = checkpoint_key(&first);
    host.prepare_flow(&first, &manifest.flows[0], first_key)
        .await
        .unwrap();

    let qualified = qualified_surface();
    let first_binding = store
        .get(&user_scope(), &qualified, 7)
        .await
        .unwrap()
        .expect("generation seven binding");
    first_binding
        .inspect(&manifest.flows[0], &package_root)
        .await
        .unwrap();

    let second = intent(&manifest, 8);
    let second_key = checkpoint_key(&second);
    host.prepare_flow(&second, &manifest.flows[0], second_key)
        .await
        .unwrap();
    assert!(store
        .get(&user_scope(), &qualified, 7)
        .await
        .unwrap()
        .is_some());
    assert!(store
        .get(&user_scope(), &qualified, 8)
        .await
        .unwrap()
        .is_some());

    host.stop_flow(&second, &manifest.flows[0], second_key)
        .await
        .unwrap();
    assert!(store
        .get(&user_scope(), &qualified, 8)
        .await
        .unwrap()
        .is_some());
    host.remove_flow(&second, &manifest.flows[0], second_key)
        .await
        .unwrap();
    assert!(store
        .get(&user_scope(), &qualified, 8)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .get(&user_scope(), &qualified, 7)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn retained_flow_binding_rejects_artifact_substitution() {
    let temporary = tempfile::tempdir().unwrap();
    let package_root = temporary.path().join("package");
    std::fs::create_dir_all(package_root.join("flows")).unwrap();
    std::fs::write(
        package_root.join("flows/review.ts"),
        "export function run() {}\n",
    )
    .unwrap();
    let compiler = fake_flow_compiler(temporary.path());

    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let store = FlowRuntimeBindingStore::new(temporary.path().join("state"));
    let host = A3sFlowLifecycleHost::new(
        &package_root,
        &compiler,
        temporary.path().join("cache"),
        store.clone(),
    );
    let intent = intent(&manifest, 9);
    host.prepare_flow(&intent, &manifest.flows[0], checkpoint_key(&intent))
        .await
        .unwrap();
    let binding = store
        .get(&user_scope(), &qualified_surface(), 9)
        .await
        .unwrap()
        .unwrap();
    std::fs::write(binding.artifact(), b"substituted").unwrap();

    let error = binding
        .inspect(&manifest.flows[0], &package_root)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.flow_artifact_changed");
}

#[tokio::test]
async fn binding_store_rejects_tampered_and_moved_records() {
    let (temporary, _package_root, _manifest, store) = prepared_fixture(10).await;
    let record = binding_record_path(&store, &user_scope(), 10);
    let original = std::fs::read(&record).unwrap();

    std::fs::write(&record, b"{\"schema\":\"tampered\"}").unwrap();
    let error = store
        .get(&user_scope(), &qualified_surface(), 10)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.flow_binding_record_invalid");
    std::fs::write(&record, &original).unwrap();

    let moved_generation = binding_record_path(&store, &user_scope(), 11);
    std::fs::write(&moved_generation, &original).unwrap();
    let error = store
        .get(&user_scope(), &qualified_surface(), 11)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.flow_binding_ownership_mismatch");

    let moved_scope = binding_record_path(&store, &workspace_scope(), 10);
    std::fs::create_dir_all(moved_scope.parent().unwrap()).unwrap();
    std::fs::write(&moved_scope, &original).unwrap();
    let error = store
        .get(&workspace_scope(), &qualified_surface(), 10)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.flow_binding_ownership_mismatch");

    drop(temporary);
}

#[tokio::test]
async fn identical_scope_ids_are_isolated_by_scope_kind() {
    let (_temporary, _package_root, _manifest, store) = prepared_fixture(11).await;
    let user = store
        .get(&user_scope(), &qualified_surface(), 11)
        .await
        .unwrap()
        .expect("user binding");
    let mut value = serde_json::to_value(&user).unwrap();
    value["scope"]["kind"] = serde_json::Value::String("workspace".to_owned());
    let workspace = serde_json::from_value::<super::FlowRuntimeBinding>(value).unwrap();

    assert!(store.put(&workspace).await.unwrap());
    assert_ne!(
        binding_record_path(&store, &user_scope(), 11),
        binding_record_path(&store, &workspace_scope(), 11)
    );
    assert_eq!(
        store
            .get(&user_scope(), &qualified_surface(), 11)
            .await
            .unwrap(),
        Some(user)
    );
    assert_eq!(
        store
            .get(&workspace_scope(), &qualified_surface(), 11)
            .await
            .unwrap(),
        Some(workspace)
    );
}

#[tokio::test]
async fn binding_store_rejects_a_linked_scope_directory() {
    let (temporary, _package_root, _manifest, store) = prepared_fixture(12).await;
    let record = binding_record_path(&store, &user_scope(), 12);
    let scope_directory = record
        .ancestors()
        .nth(4)
        .expect("scope directory beneath the binding root");
    std::fs::remove_dir_all(scope_directory).unwrap();
    let external = temporary.path().join("external-scope");
    std::fs::create_dir_all(&external).unwrap();
    crate::test_filesystem::create_directory_link(&external, scope_directory);

    let error = store
        .get(&user_scope(), &qualified_surface(), 12)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.plugin.flow_binding_path_invalid");
}

async fn prepared_fixture(
    generation: u64,
) -> (
    tempfile::TempDir,
    PathBuf,
    ExtensionManifest,
    FlowRuntimeBindingStore,
) {
    let temporary = tempfile::tempdir().unwrap();
    let package_root = temporary.path().join("package");
    std::fs::create_dir_all(package_root.join("flows")).unwrap();
    std::fs::write(
        package_root.join("flows/review.ts"),
        "export function run() { return { type: 'complete', output: {} }; }\n",
    )
    .unwrap();
    let compiler = fake_flow_compiler(temporary.path());

    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let store = FlowRuntimeBindingStore::new(temporary.path().join("state"));
    let host = A3sFlowLifecycleHost::new(
        &package_root,
        &compiler,
        temporary.path().join("cache"),
        store.clone(),
    );
    let intent = intent(&manifest, generation);
    host.prepare_flow(&intent, &manifest.flows[0], checkpoint_key(&intent))
        .await
        .unwrap();
    (temporary, package_root, manifest, store)
}

fn fake_flow_compiler(root: &Path) -> PathBuf {
    let compiler = if cfg!(windows) {
        root.join("compiler.cmd")
    } else {
        root.join("compiler")
    };
    write_fake_flow_compiler(&compiler);
    compiler
}

#[cfg(unix)]
fn write_fake_flow_compiler(compiler: &Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(
        compiler,
        r#"#!/bin/sh
set -eu
[ "$1" = "compile" ]
shift
output=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    output="$1"
  fi
  shift
done
[ -n "$output" ]
printf '#!/bin/sh\nexit 0\n' > "$output"
chmod +x "$output"
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(compiler).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(compiler, permissions).unwrap();
}

#[cfg(windows)]
fn write_fake_flow_compiler(compiler: &Path) {
    std::fs::write(
        compiler,
        "@echo off\r\n\
setlocal EnableExtensions\r\n\
if /I not \"%~1\"==\"compile\" exit /b 2\r\n\
shift\r\n\
set \"output=\"\r\n\
:parse\r\n\
if \"%~1\"==\"\" goto done\r\n\
if /I \"%~1\"==\"-o\" goto output\r\n\
:next\r\n\
shift\r\n\
goto parse\r\n\
:output\r\n\
shift\r\n\
set \"output=%~1\"\r\n\
goto next\r\n\
:done\r\n\
if not defined output exit /b 3\r\n\
> \"%output%\" echo @echo off\r\n\
>> \"%output%\" echo exit /b 0\r\n\
exit /b 0\r\n",
    )
    .unwrap();
}

fn binding_record_path(
    store: &FlowRuntimeBindingStore,
    scope: &a3s_use_core::PlanScope,
    generation: u64,
) -> PathBuf {
    let scope_digest = format!("{:x}", Sha256::digest(scope.id.as_bytes()));
    store
        .root()
        .join(scope.kind.as_str())
        .join(scope_digest)
        .join("acme")
        .join("review")
        .join("flow-review")
        .join(format!("{generation:020}.json"))
}

fn user_scope() -> a3s_use_core::PlanScope {
    a3s_use_core::PlanScope {
        kind: a3s_use_core::PlanScopeKind::User,
        id: "current".to_string(),
    }
}

fn workspace_scope() -> a3s_use_core::PlanScope {
    a3s_use_core::PlanScope {
        kind: a3s_use_core::PlanScopeKind::Workspace,
        id: "current".to_string(),
    }
}

fn intent(manifest: &ExtensionManifest, generation: u64) -> PluginLifecycleIntent {
    PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: format!("flow-install-{generation}"),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope: a3s_use_core::PlanScope {
                kind: a3s_use_core::PlanScopeKind::User,
                id: "current".to_string(),
            },
            package_id: manifest.package_id.clone(),
            package_digest: format!("sha256:{}", "2".repeat(64)),
            manifest_digest: format!("sha256:{:x}", Sha256::digest(MANIFEST.as_bytes())),
            generation,
            action: PluginLifecycleAction::Install,
            retained_ui_state_surfaces: Vec::new(),
        },
        manifest,
    )
    .unwrap()
}

fn checkpoint_key(intent: &PluginLifecycleIntent) -> &str {
    &intent
        .checkpoints
        .iter()
        .find(|checkpoint| {
            checkpoint.surface.as_ref().is_some_and(|surface| {
                surface.kind == PluginSurfaceKind::Flow && surface.id == "review"
            })
        })
        .unwrap()
        .idempotency_key
}

fn qualified_surface() -> PlanQualifiedSurfaceRef {
    PlanQualifiedSurfaceRef {
        package_id: "acme/review".to_string(),
        surface: a3s_use_core::PluginSurfaceRef {
            kind: PluginSurfaceKind::Flow,
            id: "review".to_string(),
        },
    }
}

fn _assert_paths_are_send_sync<T: Send + Sync>() {}

#[test]
fn flow_runtime_public_contracts_are_send_and_sync() {
    _assert_paths_are_send_sync::<A3sFlowLifecycleHost>();
    _assert_paths_are_send_sync::<FlowRuntimeBindingStore>();
    _assert_paths_are_send_sync::<PathBuf>();
    let _: Option<&Path> = None;
}
