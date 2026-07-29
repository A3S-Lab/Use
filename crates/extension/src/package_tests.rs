use a3s_use_core::{ToolReleaseDescriptor, ToolWorkloadContract};
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tokio::fs;

use super::package::validate_surface_files;
use super::ExtensionManifest;

const TASK_RELEASE: &[u8] =
    include_bytes!("../../core/fixtures/releases/tool-task-release-v1.json");
const SERVICE_RELEASE: &[u8] =
    include_bytes!("../../core/fixtures/releases/tool-service-release-v1.json");
const MANIFEST: &str = r#"
extension "acme/tools" {
  schema_version = 3
  version        = "1.0.0"
  route          = "tools"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["execute"]

  repository {
    url      = "https://github.com/acme/tools"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "convert" {
    workload   = "task"
    interface  = "cli"
    release    = "releases/task.json"
    command    = "acme-tools-convert"
    timeout_ms = 120000
  }

  tool "index" {
    workload  = "service"
    interface = "http"
    release   = "releases/service.json"
    base_path = "/api"
    contract  = "contracts/openapi.json"
  }
}
"#;

async fn write_package() -> (tempfile::TempDir, ExtensionManifest, ToolReleaseDescriptor) {
    let directory = tempdir().unwrap();
    let root = directory.path();
    fs::create_dir_all(root.join("releases")).await.unwrap();
    fs::create_dir_all(root.join("contracts")).await.unwrap();
    fs::write(root.join("releases/task.json"), TASK_RELEASE)
        .await
        .unwrap();

    let contract = br#"{"openapi":"3.1.0"}"#;
    fs::write(root.join("contracts/openapi.json"), contract)
        .await
        .unwrap();
    let mut service = ToolReleaseDescriptor::from_json(SERVICE_RELEASE).unwrap();
    let ToolWorkloadContract::Service {
        api_contract_digest,
        ..
    } = &mut service.workload
    else {
        panic!("fixture must describe a Tool Service");
    };
    *api_contract_digest = Some(format!("sha256:{:x}", Sha256::digest(contract)));
    fs::write(
        root.join("releases/service.json"),
        service.canonical_bytes().unwrap(),
    )
    .await
    .unwrap();

    (
        directory,
        ExtensionManifest::parse_acl(MANIFEST).unwrap(),
        service,
    )
}

#[tokio::test]
async fn validates_tool_release_class_and_manifest_binding() {
    let (directory, manifest, mut service) = write_package().await;
    let root = directory.path();

    validate_surface_files(&manifest, root).await.unwrap();

    fs::write(root.join("releases/task.json"), SERVICE_RELEASE)
        .await
        .unwrap();
    let error = validate_surface_files(&manifest, root).await.unwrap_err();
    assert_eq!(error.code, "use.extension.release_descriptor_invalid");
    assert!(error.message.contains("must declare a Task workload"));

    let mut task = ToolReleaseDescriptor::from_json(TASK_RELEASE).unwrap();
    let ToolWorkloadContract::Task { timeout_ms, .. } = &mut task.workload else {
        panic!("fixture must describe a Tool Task");
    };
    *timeout_ms = 1_000;
    fs::write(
        root.join("releases/task.json"),
        task.canonical_bytes().unwrap(),
    )
    .await
    .unwrap();
    let error = validate_surface_files(&manifest, root).await.unwrap_err();
    assert_eq!(error.code, "use.extension.release_descriptor_invalid");
    assert!(error.message.contains("timeout_ms"));

    fs::write(root.join("releases/task.json"), TASK_RELEASE)
        .await
        .unwrap();
    let ToolWorkloadContract::Service { base_path, .. } = &mut service.workload else {
        panic!("fixture must describe a Tool Service");
    };
    *base_path = "/different".to_string();
    fs::write(
        root.join("releases/service.json"),
        service.canonical_bytes().unwrap(),
    )
    .await
    .unwrap();
    let error = validate_surface_files(&manifest, root).await.unwrap_err();
    assert_eq!(error.code, "use.extension.release_descriptor_invalid");
    assert!(error.message.contains("base_path"));

    let ToolWorkloadContract::Service {
        base_path,
        api_contract_digest,
        ..
    } = &mut service.workload
    else {
        panic!("fixture must describe a Tool Service");
    };
    *base_path = "/api".to_string();
    *api_contract_digest = Some(format!("sha256:{}", "0".repeat(64)));
    fs::write(
        root.join("releases/service.json"),
        service.canonical_bytes().unwrap(),
    )
    .await
    .unwrap();
    let error = validate_surface_files(&manifest, root).await.unwrap_err();
    assert_eq!(error.code, "use.extension.release_descriptor_invalid");
    assert!(error.message.contains("api_contract_digest"));
}
