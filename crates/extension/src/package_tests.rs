use a3s_use_core::{ToolReleaseDescriptor, ToolWorkloadContract};
use sha2::{Digest, Sha256};
use tempfile::tempdir;
use tokio::fs;
use tough::{ExpirationEnforcement, RepositoryLoader};
use url::Url;

use super::digest::package_fingerprint;
use super::package::{read_manifest, validate_surface_files};
use super::remote::test_support::{package_directory_archive, TestRepository, TestServer, FUTURE};
use super::source::prepare_package_source;
use super::ExtensionManifest;

const TASK_RELEASE: &[u8] =
    include_bytes!("../../core/fixtures/releases/tool-task-release-v1.json");
const SERVICE_RELEASE: &[u8] =
    include_bytes!("../../core/fixtures/releases/tool-service-release-v1.json");
const PLUGIN_V3_MANIFEST: &[u8] = include_bytes!("../fixtures/manifests/plugin-v3.acl");
const PLUGIN_V3_PACKAGE_SHA256: &str =
    include_str!("../fixtures/packages/plugin-v3/package.sha256").trim_ascii_end();
const PLUGIN_V3_PACKAGE_STATS: &str =
    include_str!("../fixtures/packages/plugin-v3/package.stats.json").trim_ascii_end();
const COMPLETE_PACKAGE_CATALOG: &[u8] =
    include_bytes!("../../core/fixtures/plugins/complete-package-catalog-v1.json");
const TUF_ROOT: &[u8] = include_bytes!("../fixtures/registry/plugin-v3/metadata/root.json");
const TUF_TARGETS: &[u8] = include_bytes!("../fixtures/registry/plugin-v3/metadata/targets.json");
const TUF_SNAPSHOT: &[u8] = include_bytes!("../fixtures/registry/plugin-v3/metadata/snapshot.json");
const TUF_TIMESTAMP: &[u8] =
    include_bytes!("../fixtures/registry/plugin-v3/metadata/timestamp.json");
const TUF_ROOT_SHA256: &str =
    include_str!("../fixtures/registry/plugin-v3/root.sha256").trim_ascii_end();
const ARCHIVE_STATS: &str =
    include_str!("../fixtures/registry/plugin-v3/archive.stats.json").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

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

#[tokio::test]
async fn complete_plugin_v3_package_fixture_is_valid_and_content_addressed() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/packages/plugin-v3/package");
    let (manifest, manifest_bytes) = read_manifest(&root).await.unwrap();
    assert_eq!(manifest_bytes, PLUGIN_V3_MANIFEST);
    assert_eq!(manifest.package_id, "acme/research");
    assert_eq!(manifest.tools.len(), 2);
    assert_eq!(manifest.mcp_servers.len(), 2);
    assert_eq!(manifest.skills.len(), 2);
    assert_eq!(manifest.ui.len(), 2);
    validate_surface_files(&manifest, &root).await.unwrap();

    let fingerprint = package_fingerprint(&root).await.unwrap();
    assert_eq!(
        format!("sha256:{}", fingerprint.sha256),
        PLUGIN_V3_PACKAGE_SHA256
    );
    assert_eq!(
        serde_json::json!({
            "byteCount": fingerprint.byte_count,
            "fileCount": fingerprint.file_count,
            "sha256": format!("sha256:{}", fingerprint.sha256),
        }),
        serde_json::from_str::<serde_json::Value>(PLUGIN_V3_PACKAGE_STATS).unwrap()
    );
    let archive = package_directory_archive(&root);
    let catalog: serde_json::Value = serde_json::from_slice(COMPLETE_PACKAGE_CATALOG).unwrap();
    assert_eq!(catalog["archive"]["length"], archive.len());
    assert_eq!(
        catalog["archive"]["sha256"],
        format!("sha256:{:x}", Sha256::digest(&archive))
    );
    let archive_temp = tempdir().unwrap();
    let archive_path = archive_temp.path().join("plugin-v3.tar.gz");
    fs::write(&archive_path, &archive).await.unwrap();
    let extracted = prepare_package_source(&archive_path).await.unwrap();
    let (extracted_manifest, _) = read_manifest(extracted.root()).await.unwrap();
    validate_surface_files(&extracted_manifest, extracted.root())
        .await
        .unwrap();
    assert_eq!(
        package_fingerprint(extracted.root()).await.unwrap(),
        fingerprint
    );

    let target_name = catalog["archive"]["targetName"]
        .as_str()
        .unwrap()
        .to_owned();
    let repository = TestRepository::with_target_metadata(archive, target_name, catalog, 7, FUTURE);
    for (route, fixture) in [
        ("/metadata/root.json", TUF_ROOT),
        ("/metadata/targets.json", TUF_TARGETS),
        ("/metadata/snapshot.json", TUF_SNAPSHOT),
        ("/metadata/timestamp.json", TUF_TIMESTAMP),
    ] {
        assert_eq!(
            repository.routes.get(route).unwrap(),
            canonical_fixture(fixture)
        );
    }
    assert_eq!(
        format!("sha256:{}", repository.root_sha256),
        TUF_ROOT_SHA256
    );
    assert_eq!(
        serde_json::json!({
            "length": repository
                .routes
                .get(&format!("/targets/{}", repository.target_name))
                .unwrap()
                .len(),
            "sha256": format!("sha256:{}", repository.target_sha256),
            "targetName": repository.target_name,
        }),
        serde_json::from_str::<serde_json::Value>(ARCHIVE_STATS).unwrap()
    );

    let server = TestServer::start(repository.routes.clone());
    let metadata_url = Url::parse(&format!("{}metadata/", server.base_url())).unwrap();
    let targets_url = Url::parse(&format!("{}targets/", server.base_url())).unwrap();
    let verified = RepositoryLoader::new(&canonical_fixture(TUF_ROOT), metadata_url, targets_url)
        .expiration_enforcement(ExpirationEnforcement::Safe)
        .load()
        .await
        .unwrap();
    let (verified_name, verified_target) = verified.all_targets().next().unwrap();
    assert_eq!(verified_name.raw(), repository.target_name);
    let signed_catalog = verified_target.custom.get("a3s").unwrap();
    assert_eq!(
        signed_catalog,
        &serde_json::from_slice::<serde_json::Value>(COMPLETE_PACKAGE_CATALOG).unwrap()
    );
}
