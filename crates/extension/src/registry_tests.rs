use std::time::Duration;

use super::*;
use crate::package::write_receipt;

#[path = "registry_tests/artifact_admission.rs"]
mod artifact_admission;
#[path = "registry_tests/cognitive_lifecycle.rs"]
mod cognitive_lifecycle;
#[path = "registry_tests/cutover.rs"]
mod cutover;
#[path = "registry_tests/graph_cutover.rs"]
mod graph_cutover;
#[path = "registry_tests/lifecycle_generations.rs"]
mod lifecycle_generations;
#[path = "registry_tests/lifecycle_staging.rs"]
mod lifecycle_staging;
#[cfg(windows)]
#[path = "registry_tests/lifecycle_windows_contention.rs"]
mod lifecycle_windows_contention;
#[path = "registry_tests/scope_isolation.rs"]
mod scope_isolation;
#[path = "registry_tests/snapshot_lease.rs"]
mod snapshot_lease;

const MANIFEST_NAME: &str = "a3s-use-extension.acl";

#[test]
fn published_generation_lease_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ExtensionGenerationLease>();
    assert_send_sync::<ExtensionSnapshotLease>();
    assert_send_sync::<ExtensionArtifactReference>();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_change_observes_the_atomic_registry_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let registry = registry(temporary.path());
    let initial = registry.snapshot().await.unwrap();
    let observer = registry.clone();
    let wait = tokio::spawn(async move {
        observer
            .wait_for_change(initial.generation, Duration::from_secs(10))
            .await
    });

    let published = ExtensionRegistrySnapshot {
        schema_version: REGISTRY_SCHEMA_VERSION,
        installation: registry.installation().clone(),
        generation: initial.generation + 1,
        packages: Vec::new(),
        pending_cutovers: Vec::new(),
    };
    crate::registry_io::write_registry_snapshot(registry.paths(), &published)
        .await
        .unwrap();

    let changed = tokio::time::timeout(Duration::from_secs(15), wait)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(changed, published);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wait_for_change_times_out_without_a_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let registry = registry(temporary.path());
    let initial = registry.snapshot().await.unwrap();

    let changed = tokio::time::timeout(
        Duration::from_secs(5),
        registry.wait_for_change(initial.generation, Duration::from_millis(25)),
    )
    .await
    .unwrap()
    .unwrap();

    assert!(changed.is_none());
}

#[tokio::test]
async fn receipt_writer_rejects_an_oversized_record_before_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let registry = registry(temporary.path());
    let artifact_store = registry.paths().artifact_store();
    let digest = format!("sha256:{}", "a".repeat(64));
    let receipt = ExtensionReceipt {
        schema_version: EXTENSION_RECEIPT_SCHEMA_VERSION,
        installation: registry.installation().clone(),
        package_id: "acme/oversized".to_owned(),
        component_id: "use/acme/oversized".to_owned(),
        route_alias: None,
        version: "1.0.0".to_owned(),
        package_root: artifact_store.expanded_package_path(&digest).unwrap(),
        manifest_sha256: "b".repeat(64),
        package_sha256: Some("a".repeat(64)),
        trust: ExtensionTrust::LocalExplicit,
        registry: None,
        verified_catalog: None,
        planning_bundle: None,
        selected_surfaces: vec![PluginSurfaceRef {
            kind: PluginSurfaceKind::Skill,
            id: "x".repeat(MAX_EXTENSION_RECEIPT_BYTES as usize),
        }],
        installed_at_unix: 1,
        enabled: true,
        lifecycle_generation: Some(1),
    };
    let path = temporary.path().join("oversized-receipt.json");
    let admission = artifact_store.acquire_reference_admission().await.unwrap();

    let error = write_receipt(&artifact_store, &admission, &path, &receipt)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.extension.receipt_invalid");
    assert!(!path.exists());
    assert!(std::fs::read_dir(temporary.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".receipt-")));
}

fn registry(root: &Path) -> ExtensionRegistry {
    ExtensionRegistry::new(
        ExtensionPaths::new(
            root.join("data"),
            root.join("state"),
            a3s_use_core::InstallationId::new(
                a3s_use_core::InstallationKind::User,
                "extension-tests",
            )
            .unwrap(),
        )
        .unwrap(),
    )
}

async fn compatible_cognitive_package(root: &Path) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/packages/plugin-v3-cognitive/package");
    crate::package::copy_package(&fixture, root).await.unwrap();
}

async fn cognitive_package_with_dependencies(
    root: &Path,
    package_id: &str,
    route: &str,
    dependencies: &[(&str, &str)],
) {
    compatible_cognitive_package(root).await;
    let path = root.join(MANIFEST_NAME);
    let mut manifest = fs::read_to_string(&path).await.unwrap();
    manifest = manifest
        .replace(
            "extension \"acme/cognitive\"",
            &format!("extension \"{package_id}\""),
        )
        .replace(
            "route          = \"cognitive\"",
            &format!("route          = \"{route}\""),
        );
    let dependency_blocks = dependencies
        .iter()
        .map(|(dependency, requirement)| {
            format!("  dependency \"{dependency}\" {{\n    version = \"{requirement}\"\n  }}\n\n")
        })
        .collect::<String>();
    manifest = manifest.replace(
        "  repository {",
        &format!("{dependency_blocks}  repository {{"),
    );
    fs::write(path, manifest).await.unwrap();
}

async fn knowledge_package_with_dependencies(
    root: &Path,
    package_id: &str,
    route: &str,
    dependencies: &[(&str, &str)],
) {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    crate::package::copy_package(&fixture, root).await.unwrap();
    let path = root.join(MANIFEST_NAME);
    let mut manifest = fs::read_to_string(&path).await.unwrap();
    manifest = manifest
        .replace(
            "extension \"acme/knowledge\"",
            &format!("extension \"{package_id}\""),
        )
        .replace(
            "route          = \"knowledge\"",
            &format!("route          = \"{route}\""),
        );
    let dependency_blocks = dependencies
        .iter()
        .map(|(dependency, requirement)| {
            format!("  dependency \"{dependency}\" {{\n    version = \"{requirement}\"\n  }}\n\n")
        })
        .collect::<String>();
    manifest = manifest.replace(
        "  repository {",
        &format!("{dependency_blocks}  repository {{"),
    );
    fs::write(path, manifest).await.unwrap();
}

async fn verified_knowledge_catalog(
    root: &Path,
    package_id: &str,
    dependencies: &[(&str, &str)],
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let (_, manifest_bytes) = read_manifest(root).await.unwrap();
    let fingerprint = crate::digest::package_fingerprint(root).await.unwrap();
    let mut catalog = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    catalog.package_id = package_id.to_string();
    catalog.publisher = publisher.to_string();
    catalog.display_name = format!("{publisher} {name}");
    catalog.description = format!("Lifecycle graph fixture for {package_id}.");
    catalog.repository = format!("https://github.com/{publisher}/{name}");
    catalog.target = "any".to_string();
    catalog.dependencies = dependencies
        .iter()
        .map(|(dependency, requirement)| {
            a3s_use_core::PluginPackageDependency::new(*dependency, *requirement).unwrap()
        })
        .collect();
    catalog.archive.target_name =
        format!("extensions/{package_id}/1.0.0/stable/any/{publisher}-{name}-1.0.0-any.tar.gz");
    catalog.archive.length = 1;
    catalog.archive.sha256 = format!("sha256:{}", seed.to_string().repeat(64));
    catalog.package.expanded_bytes = fingerprint.byte_count;
    catalog.package.file_count = fingerprint.file_count;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.sha256));
    catalog.package.manifest_sha256 = Some(format!("sha256:{}", sha256(&manifest_bytes)));
    catalog.validate().unwrap();
    let provenance = a3s_use_core::VerifiedCatalogProvenance {
        registry_name: "fixture".to_string(),
        registry_url: "https://packages.example.test/catalog/".to_string(),
        root_sha256: format!("sha256:{}", "f".repeat(64)),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: catalog.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(catalog, provenance).unwrap()
}

async fn bind_remote_catalog_receipt(
    registry: &ExtensionRegistry,
    package_id: &str,
    catalog: &VerifiedPluginCatalogRecord,
) {
    let mut receipt = registry.get(package_id).await.unwrap().unwrap().receipt;
    receipt.trust = ExtensionTrust::RegistryTuf;
    receipt.registry = Some(ResolvedRemotePackage::from_verified_catalog(catalog).unwrap());
    receipt.verified_catalog = Some(catalog.clone());
    let artifact_store = registry.paths().artifact_store();
    let artifact_admission = artifact_store.acquire_reference_admission().await.unwrap();
    write_receipt(
        &artifact_store,
        &artifact_admission,
        &registry.paths().receipt_path(package_id),
        &receipt,
    )
    .await
    .unwrap();
}

fn lifecycle_identity(
    candidate: &ExtensionLifecyclePackage,
    generation: u64,
) -> ExtensionLifecycleIdentity {
    ExtensionLifecycleIdentity::new(
        candidate.package_id(),
        candidate.package_digest(),
        candidate.manifest_digest(),
        generation,
    )
    .unwrap()
}
