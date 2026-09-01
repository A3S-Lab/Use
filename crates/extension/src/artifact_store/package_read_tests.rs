use std::path::{Path, PathBuf};

use a3s_use_core::{
    CatalogSurface, PluginCatalogRecord, VerifiedCatalogProvenance, VerifiedPluginCatalogRecord,
};
use fs2::FileExt;

use super::*;
use crate::package::{copy_package, sha256, MANIFEST_NAME};

#[tokio::test]
async fn verified_package_lease_binds_catalog_identity_and_blocks_mutation() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, package_root) = stage_fixture(&fixture).await;

    let package = store.acquire_verified_package(&catalog).await.unwrap();

    assert_eq!(package.package_id(), "acme/knowledge");
    assert_eq!(package.version(), "1.0.0");
    assert_eq!(
        package.package_digest(),
        catalog.record.package.sha256.as_deref().unwrap()
    );
    assert_eq!(
        package.manifest_digest(),
        catalog.record.package.manifest_sha256.as_deref().unwrap()
    );
    assert_eq!(
        package.expanded_bytes(),
        catalog.record.package.expanded_bytes
    );
    assert_eq!(package.file_count(), catalog.record.package.file_count);
    assert_eq!(package.manifest().package_id, "acme/knowledge");
    let store_root = store.root().to_string_lossy();
    assert!(!format!("{package:?}").contains(store_root.as_ref() as &str));
    package.verify_unchanged().await.unwrap();

    let mutation = open_lock_file(
        &package_root.parent().unwrap().join(MUTATION_LOCK),
        "test artifact mutation",
    )
    .unwrap();
    assert!(mutation.try_lock_exclusive().is_err());
    assert_eq!(
        store.acquire_collection().await.unwrap_err().code,
        "use.artifact_store.busy"
    );

    drop(package);
    mutation.try_lock_exclusive().unwrap();
    FileExt::unlock(&mutation).unwrap();
    drop(store.acquire_collection().await.unwrap());
}

#[tokio::test]
async fn verified_package_lease_inspects_one_named_skill_without_exposing_its_path() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, _) = stage_fixture(&fixture).await;
    let package = store.acquire_verified_package(&catalog).await.unwrap();

    let evidence = package.inspect_skill_surface("research").await.unwrap();

    assert_eq!(evidence.file_count(), 1);
    assert!(evidence.expanded_bytes() > 0);
    assert!(evidence.digest().starts_with("sha256:"));
    assert_eq!(evidence.digest().len(), 71);

    let error = package.inspect_skill_surface("missing").await.unwrap_err();
    assert_eq!(error.code, "use.artifact_store.surface_missing");
}

#[tokio::test]
async fn verified_package_lease_inspects_one_named_ui_snapshot() {
    let fixture = static_package_fixture();
    let catalog = catalog_for_static_fixture(&fixture).await;
    let (_temporary, store, _, _) = stage_fixture_with_catalog(&fixture, catalog.clone()).await;
    let package = store.acquire_verified_package(&catalog).await.unwrap();

    let skill = package.inspect_skill_surface("guide").await.unwrap();
    let ui = package.inspect_ui_surface("panel").await.unwrap();

    assert_eq!(skill.file_count(), 1);
    assert_eq!(ui.file_count(), 3);
    assert!(ui.expanded_bytes() > 0);
    assert_ne!(skill.digest(), ui.digest());
}

#[tokio::test]
async fn verified_package_lease_reads_one_named_okf_snapshot_without_exposing_its_path() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, _) = stage_fixture(&fixture).await;
    let package = store.acquire_verified_package(&catalog).await.unwrap();

    let payload = package.read_okf_surface("domain-knowledge").await.unwrap();

    assert_eq!(payload.surface_id(), "domain-knowledge");
    assert_eq!(
        payload.bundle(),
        package
            .manifest()
            .okf
            .iter()
            .find(|surface| surface.id == "domain-knowledge")
            .map(|surface| &surface.bundle)
            .unwrap()
    );
    assert_eq!(payload.files().len() as u64, payload.bundle().file_count);
    assert!(payload.files().iter().all(|file| {
        let path = Path::new(&file.path);
        !path.is_absolute() && !file.path.contains("..")
    }));

    let error = package.read_okf_surface("missing").await.unwrap_err();
    assert_eq!(error.code, "use.artifact_store.surface_missing");
}

#[tokio::test]
async fn verified_package_lease_fails_closed_for_content_tampering() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, package_root) = stage_fixture(&fixture).await;
    std::fs::write(package_root.join("README.md"), b"substituted").unwrap();

    let error = store.acquire_verified_package(&catalog).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.package_mismatch");
}

#[tokio::test]
async fn active_verified_package_lease_detects_external_tampering() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, package_root) = stage_fixture(&fixture).await;
    let package = store.acquire_verified_package(&catalog).await.unwrap();
    std::fs::write(package_root.join("README.md"), b"substituted").unwrap();

    let error = package.verify_unchanged().await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.package_mismatch");
}

#[tokio::test]
async fn verified_package_lease_rejects_catalog_measurement_drift() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, _) = stage_fixture(&fixture).await;
    let mut record = catalog.record;
    record.package.expanded_bytes += 1;
    let catalog = verified_catalog(record);

    let error = store.acquire_verified_package(&catalog).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.package_mismatch");
}

#[tokio::test]
async fn verified_package_lease_rejects_logically_quarantined_content() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, package_root) = stage_fixture(&fixture).await;
    std::fs::write(package_root.join("README.md"), b"substituted").unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let digest = catalog.record.package.sha256.as_deref().unwrap();
    let plan = store
        .plan_quarantine(&collection, ArtifactKind::ExpandedPackage, digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::ExpandedPackage,
            digest,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    drop(collection);

    let error = store.acquire_verified_package(&catalog).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quarantined");
}

#[tokio::test]
async fn verified_package_read_does_not_create_a_missing_mutation_lock() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, package_root) = stage_fixture(&fixture).await;
    let lock = package_root.parent().unwrap().join(MUTATION_LOCK);
    std::fs::remove_file(&lock).unwrap();

    let error = store.acquire_verified_package(&catalog).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.content_missing");
    assert!(!lock.exists());
}

#[tokio::test]
async fn interrupted_garbage_collection_fences_verified_package_reads() {
    let fixture = package_fixture();
    let (_temporary, store, catalog, _) = stage_fixture(&fixture).await;
    std::fs::write(
        store
            .root()
            .join(super::garbage_collection::GARBAGE_COLLECTION_PREPARED_TEMPORARY),
        b"",
    )
    .unwrap();

    let error = store.acquire_verified_package(&catalog).await.unwrap_err();

    assert_eq!(
        error.code,
        "use.artifact_store.garbage_collection_in_progress"
    );
}

#[tokio::test]
async fn manifest_file_reads_are_bounded_before_acl_parsing() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("package");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join(MANIFEST_NAME),
        vec![b'x'; crate::package::MAX_EXTENSION_MANIFEST_BYTES as usize + 1],
    )
    .unwrap();

    let error = crate::package::read_manifest(&root).await.unwrap_err();

    assert_eq!(error.code, "use.extension.manifest_too_large");
}

async fn stage_fixture(
    fixture: &Path,
) -> (
    tempfile::TempDir,
    ArtifactStore,
    VerifiedPluginCatalogRecord,
    PathBuf,
) {
    let catalog = catalog_for_fixture(fixture).await;
    stage_fixture_with_catalog(fixture, catalog).await
}

async fn stage_fixture_with_catalog(
    fixture: &Path,
    catalog: VerifiedPluginCatalogRecord,
) -> (
    tempfile::TempDir,
    ArtifactStore,
    VerifiedPluginCatalogRecord,
    PathBuf,
) {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = catalog.record.package.sha256.as_deref().unwrap();
    let package_root = store.expanded_package_path(digest).unwrap();
    let admission = store.acquire_reference_admission().await.unwrap();
    std::fs::create_dir_all(package_root.parent().unwrap()).unwrap();
    open_lock_file(
        &package_root.parent().unwrap().join(MUTATION_LOCK),
        "fixture artifact mutation",
    )
    .unwrap();
    copy_package(fixture, &package_root).await.unwrap();
    drop(admission);
    (temporary, store, catalog, package_root)
}

async fn catalog_for_fixture(root: &Path) -> VerifiedPluginCatalogRecord {
    let (_, manifest_bytes) = crate::package::read_manifest(root).await.unwrap();
    let fingerprint = crate::digest::package_fingerprint(root).await.unwrap();
    let mut record = PluginCatalogRecord::from_json(include_bytes!(
        "../../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    record.package.expanded_bytes = fingerprint.byte_count;
    record.package.file_count = fingerprint.file_count;
    record.package.sha256 = Some(format!("sha256:{}", fingerprint.sha256));
    record.package.manifest_sha256 = Some(format!("sha256:{}", sha256(&manifest_bytes)));
    verified_catalog(record)
}

async fn catalog_for_static_fixture(root: &Path) -> VerifiedPluginCatalogRecord {
    let (manifest, manifest_bytes) = crate::package::read_manifest(root).await.unwrap();
    let fingerprint = crate::digest::package_fingerprint(root).await.unwrap();
    let mut record = PluginCatalogRecord::from_json(include_bytes!(
        "../../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    record.package_id = manifest.package_id.clone();
    record.display_name = "Static Surface Fixture".to_string();
    record.description = "Path-free static surface lease fixture.".to_string();
    record.version = manifest.version.clone();
    record.dependencies = manifest.dependencies.clone();
    record.surfaces = manifest
        .plugin_surfaces()
        .unwrap()
        .into_iter()
        .map(|surface| CatalogSurface {
            kind: surface.surface.kind,
            id: surface.surface.id,
            optional: surface.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: None,
            requires: surface.dependencies,
        })
        .collect();
    record.archive.target_name =
        "extensions/acme/static/1.0.0/stable/linux-x86_64/acme-static-1.0.0-linux-x86_64.tar.gz"
            .to_string();
    record.package.expanded_bytes = fingerprint.byte_count;
    record.package.file_count = fingerprint.file_count;
    record.package.sha256 = Some(format!("sha256:{}", fingerprint.sha256));
    record.package.manifest_sha256 = Some(format!("sha256:{}", sha256(&manifest_bytes)));
    verified_catalog(record)
}

fn verified_catalog(record: PluginCatalogRecord) -> VerifiedPluginCatalogRecord {
    record.validate().unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "fixture".to_string(),
        registry_url: "https://packages.example.test/catalog/".to_string(),
        root_sha256: format!("sha256:{}", "f".repeat(64)),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(record, provenance).unwrap()
}

fn package_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package")
}

fn static_package_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-static/package")
}
