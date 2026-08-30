use a3s_use_core::{
    CatalogAvailability, InstallationKind, InstallationPackageSelection, PluginCatalogRecord,
    PluginPackageLock, PluginPackageLockHost, PluginPackageResolver, VerifiedCatalogProvenance,
    VerifiedPluginCatalogRecord, PLUGIN_CATALOG_SCHEMA_V3,
};
use a3s_use_extension::{
    ExtensionManifest, ExtensionPaths, ExtensionReceipt, ExtensionTrust,
    EXTENSION_RECEIPT_SCHEMA_VERSION,
};
use sha2::{Digest, Sha256};
use tokio::fs;

use super::*;
use crate::cognitive_package::InstallationSnapshotStore;
use crate::plugin_lifecycle::{
    PluginLifecycleAction, PluginLifecycleIntent, PluginLifecycleIntentSpec,
    PluginLifecycleJournalStore,
};

const CATALOG: &[u8] =
    include_bytes!("../../crates/core/fixtures/plugins/catalog-record-okf-v3.json");
const MANIFEST: &str = include_str!("../../crates/extension/fixtures/manifests/plugin-v3-okf.acl");

mod joined_tests;

fn assert_send_sync<T: Send + Sync>() {}

async fn publish_registry_blob_reference(paths: &UsePaths, sha256: &str, digest: &str) {
    let datastore = paths
        .state_root()
        .join("remote-registries/packages/sources")
        .join("1".repeat(64));
    let cache = datastore.join("verified-targets/sha256");
    fs::create_dir_all(&cache).await.unwrap();
    fs::write(datastore.join(".target-cache.lock"), b"")
        .await
        .unwrap();
    fs::write(
        cache.join(format!("{sha256}.json")),
        format!(
            "{{\"schema\":\"a3s.use.registry-target-observation.v1\",\"targetDigest\":\"{digest}\",\"expectedBytes\":4}}"
        ),
    )
    .await
    .unwrap();
}

#[test]
fn public_reachability_types_are_send_sync() {
    assert_send_sync::<ArtifactReferenceSource>();
    assert_send_sync::<ArtifactReferenceEntry>();
    assert_send_sync::<ArtifactReferenceInventory>();
    assert_send_sync::<ArtifactReachabilityInspector>();
    assert_send_sync::<ArtifactStoreMaintenance>();
}

#[tokio::test]
async fn rehydration_coordinator_rejects_a_durable_registry_reference() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let store = roots.artifact_store();
    let candidate = temporary.path().join("verified-blob");
    fs::write(&candidate, b"good").await.unwrap();
    let sha256 = format!("{:x}", Sha256::digest(b"good"));
    let digest = format!("sha256:{sha256}");
    let content = store.blob_path(&digest).unwrap();
    fs::create_dir_all(content.parent().unwrap()).await.unwrap();
    fs::write(&content, b"evil").await.unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    drop(collection);

    publish_registry_blob_reference(&roots, &sha256, &digest).await;

    let error = ArtifactStoreMaintenance::new(roots)
        .plan_rehydration(ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_rehydration.referenced");
    assert_eq!(error.details.get("referenceCount").unwrap(), "1");
    assert_eq!(fs::read(content).await.unwrap(), b"evil");
}

#[tokio::test]
async fn rehydration_coordinator_keeps_zero_reference_proof_and_mutation_under_one_guard() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let store = roots.artifact_store();
    let candidate = temporary.path().join("verified-blob");
    fs::write(&candidate, b"good").await.unwrap();
    let sha256 = format!("{:x}", Sha256::digest(b"good"));
    let digest = format!("sha256:{sha256}");
    let content = store.blob_path(&digest).unwrap();
    fs::create_dir_all(content.parent().unwrap()).await.unwrap();
    fs::write(&content, b"evil").await.unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    drop(collection);
    let coordinator = ArtifactStoreMaintenance::new(roots.clone());
    let plan = coordinator
        .plan_rehydration(ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();

    let result = coordinator
        .apply_rehydration(
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert!(result.changed);
    assert_eq!(fs::read(content).await.unwrap(), b"good");

    fs::remove_file(&candidate).await.unwrap();
    publish_registry_blob_reference(&roots, &sha256, &digest).await;
    let replay = coordinator
        .apply_rehydration(
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert!(!replay.changed);
    assert_eq!(replay.record, result.record);
}

#[test]
fn accumulator_is_deterministic_and_counts_equal_facts() {
    let installation = InstallationId::new(
        a3s_use_core::InstallationKind::User,
        "artifact-reachability-test",
    )
    .unwrap();
    let mut accumulator = ReferenceAccumulator::default();
    let reference = RawArtifactReference {
        kind: ArtifactKind::ExpandedPackage,
        digest: format!("sha256:{}", "a".repeat(64)),
        source: ArtifactReferenceSource::InstallationSnapshot,
        installation: Some(installation),
        expected_bytes: Some(17),
        expected_files: Some(2),
    };
    accumulator.observe(reference.clone()).unwrap();
    accumulator.observe(reference).unwrap();

    let inventory = accumulator.finish();
    assert_eq!(inventory.schema, ARTIFACT_REFERENCE_INVENTORY_SCHEMA);
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].reference_count, 2);
}

#[test]
fn accumulator_rejects_conflicting_physical_expectations() {
    let installation = InstallationId::new(
        a3s_use_core::InstallationKind::Workspace,
        "artifact-reachability-test",
    )
    .unwrap();
    let reference = |expected_bytes| RawArtifactReference {
        kind: ArtifactKind::ExpandedPackage,
        digest: format!("sha256:{}", "b".repeat(64)),
        source: ArtifactReferenceSource::CurrentReceipt,
        installation: Some(installation.clone()),
        expected_bytes: Some(expected_bytes),
        expected_files: Some(1),
    };
    let mut accumulator = ReferenceAccumulator::default();
    accumulator.observe(reference(10)).unwrap();
    let error = accumulator.observe(reference(11)).unwrap_err();
    assert_eq!(error.code, "use.artifact_reachability.reference_invalid");
}

#[test]
fn accumulator_rejects_cross_source_physical_conflicts() {
    let installation =
        InstallationId::new(a3s_use_core::InstallationKind::User, "cross-source").unwrap();
    let mut accumulator = ReferenceAccumulator::default();
    accumulator
        .observe(RawArtifactReference {
            kind: ArtifactKind::ExpandedPackage,
            digest: format!("sha256:{}", "d".repeat(64)),
            source: ArtifactReferenceSource::InstallationSnapshot,
            installation: Some(installation.clone()),
            expected_bytes: Some(10),
            expected_files: Some(1),
        })
        .unwrap();
    let error = accumulator
        .observe(RawArtifactReference {
            kind: ArtifactKind::ExpandedPackage,
            digest: format!("sha256:{}", "d".repeat(64)),
            source: ArtifactReferenceSource::CurrentReceipt,
            installation: Some(installation),
            expected_bytes: Some(11),
            expected_files: Some(1),
        })
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_reachability.reference_invalid");
}

#[tokio::test]
async fn global_inventory_joins_path_free_references_across_installations() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let user = InstallationId::new(InstallationKind::User, "reachability-user").unwrap();
    let workspace =
        InstallationId::new(InstallationKind::Workspace, "reachability-workspace").unwrap();
    let package_lock = package_lock('a');
    let digest = package_lock.packages[0]
        .catalog
        .record
        .package
        .sha256
        .clone()
        .unwrap();

    for installation in [&user, &workspace] {
        let paths = roots.for_installation(installation.clone()).unwrap();
        let snapshot = InstallationSnapshotStore::from_extension_paths(&paths);
        snapshot
            .put(&package_lock, 1, package_selections(&package_lock, 1))
            .await
            .unwrap();
        write_receipt_fixture(&paths, &digest, false).await;
    }
    let user_paths = roots.for_installation(user.clone()).unwrap();
    write_receipt_fixture(&user_paths, &digest, true).await;
    let manifest = ExtensionManifest::parse_acl(MANIFEST).unwrap();
    let lifecycle = PluginLifecycleIntent::from_manifest(
        PluginLifecycleIntentSpec {
            operation_id: "install:acme-knowledge:reachability".to_owned(),
            plan_digest: format!("sha256:{}", "1".repeat(64)),
            scope: user.clone(),
            package_id: "acme/knowledge".to_owned(),
            package_digest: digest.clone(),
            manifest_digest: format!("sha256:{}", "2".repeat(64)),
            generation: 7,
            action: PluginLifecycleAction::Install,
            retained_ui_state_surfaces: Vec::new(),
        },
        &manifest,
    )
    .unwrap();
    PluginLifecycleJournalStore::from_extension_paths(&user_paths)
        .begin(&lifecycle)
        .await
        .unwrap();

    let artifact_store = roots.artifact_store();
    assert!(!artifact_store
        .expanded_package_path(&digest)
        .unwrap()
        .exists());
    let inspector = ArtifactReachabilityInspector::new(roots);
    let first = inspector.inspect_references().await.unwrap();
    let second = inspector.inspect_references().await.unwrap();
    assert_eq!(first, second);
    assert_eq!(first.schema, ARTIFACT_REFERENCE_INVENTORY_SCHEMA);

    for installation in [&user, &workspace] {
        assert!(contains_reference(
            &first,
            ArtifactReferenceSource::InstallationSnapshot,
            installation,
            &digest,
        ));
        assert!(contains_reference(
            &first,
            ArtifactReferenceSource::CurrentReceipt,
            installation,
            &digest,
        ));
    }
    assert!(contains_reference(
        &first,
        ArtifactReferenceSource::RetainedReceipt,
        &user,
        &digest,
    ));
    assert!(contains_reference(
        &first,
        ArtifactReferenceSource::PluginLifecycleOperation,
        &user,
        &digest,
    ));
    assert_eq!(first.entries.len(), 6);
    let package = &package_lock.packages[0].catalog.record.package;
    assert!(first.entries.iter().all(|entry| {
        entry.expected_bytes == Some(package.expanded_bytes)
            && entry.expected_files == Some(package.file_count)
    }));

    let value = serde_json::to_value(first).unwrap();
    let object = value.as_object().unwrap();
    assert_eq!(object.len(), 2);
    assert!(!value.to_string().contains("packageRoot"));

    let joined = inspector.inspect_reachability().await.unwrap();
    assert_eq!(joined.schema, ARTIFACT_REACHABILITY_INVENTORY_SCHEMA);
    assert_eq!(joined.artifacts.len(), 1);
    assert_eq!(joined.artifacts[0].digest, digest);
    assert_eq!(joined.artifacts[0].references.len(), 6);
    assert!(joined.artifacts[0].physical.is_none());
    assert_eq!(
        joined.artifacts[0].measurement_status,
        ArtifactMeasurementStatus::Unavailable
    );
    assert_eq!(joined.usage.artifact_keys, 1);
    assert_eq!(joined.usage.referenced_artifacts, 1);
    assert_eq!(joined.usage.physical_artifacts, 0);
    assert_eq!(joined.usage.missing_referenced_artifacts, 1);
    assert!(!serde_json::to_string(&joined)
        .unwrap()
        .contains("packageRoot"));
}

#[tokio::test]
async fn joined_inventory_reports_unreferenced_physical_content_without_authorizing_deletion() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let digest = digest('9');
    let content = roots
        .artifact_store()
        .expanded_package_path(&digest)
        .unwrap();
    fs::create_dir_all(&content).await.unwrap();
    fs::write(content.join("payload.bin"), b"data")
        .await
        .unwrap();

    let inventory = ArtifactReachabilityInspector::new(roots)
        .inspect_reachability()
        .await
        .unwrap();

    assert_eq!(inventory.artifacts.len(), 1);
    let artifact = &inventory.artifacts[0];
    assert_eq!(artifact.kind, ArtifactKind::ExpandedPackage);
    assert_eq!(artifact.digest, digest);
    assert!(artifact.references.is_empty());
    assert_eq!(
        artifact.physical.as_ref().unwrap().state,
        a3s_use_extension::ArtifactPhysicalState::Complete
    );
    assert_eq!(
        artifact.measurement_status,
        ArtifactMeasurementStatus::Unspecified
    );
    assert_eq!(inventory.usage.unreferenced_artifacts, 1);
    assert_eq!(inventory.usage.unreferenced_content_bytes, 4);
    assert!(serde_json::to_value(inventory)
        .unwrap()
        .get("deletionAuthorized")
        .is_none());
}

#[tokio::test]
async fn global_inventory_fails_closed_on_unknown_installation_state() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let installation = InstallationId::new(InstallationKind::User, "unknown-state").unwrap();
    let paths = roots.for_installation(installation).unwrap();
    fs::create_dir_all(paths.installation_state_root().join("future-authority"))
        .await
        .unwrap();

    let error = ArtifactReachabilityInspector::new(roots)
        .inspect_references()
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_reachability.reference_invalid");
}

#[tokio::test]
async fn global_inventory_rejects_an_active_state_restore() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let installation = InstallationId::new(InstallationKind::Workspace, "active-restore").unwrap();
    let paths = roots.for_installation(installation).unwrap();
    fs::create_dir_all(paths.installation_state_root())
        .await
        .unwrap();
    fs::write(
        paths
            .installation_state_root()
            .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER),
        b"active",
    )
    .await
    .unwrap();

    let error = ArtifactReachabilityInspector::new(roots)
        .inspect_references()
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_reachability.state_unstable");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn global_inventory_rejects_a_linked_installation_root() {
    let temporary = tempfile::tempdir().unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let installation = InstallationId::new(InstallationKind::User, "linked-state").unwrap();
    let state_root = roots
        .state_root()
        .join("installations/user")
        .join(installation.storage_key().unwrap());
    fs::create_dir_all(state_root.parent().unwrap())
        .await
        .unwrap();
    let external = temporary.path().join("external-installation");
    fs::create_dir_all(&external).await.unwrap();
    crate::test_filesystem::create_directory_link(&external, &state_root);

    let error = ArtifactReachabilityInspector::new(roots)
        .inspect_references()
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_reachability.reference_invalid");
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}

fn package_lock(seed: char) -> PluginPackageLock {
    let mut record = PluginCatalogRecord::from_json(CATALOG).unwrap();
    record.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    record.package_id = "acme/knowledge".to_owned();
    record.publisher = "acme".to_owned();
    record.display_name = "Knowledge reachability fixture".to_owned();
    record.description = "Artifact reachability fixture package.".to_owned();
    record.repository = "https://github.com/acme/knowledge".to_owned();
    record.archive.sha256 = digest('b');
    record.package.sha256 = Some(digest(seed));
    record.package.manifest_sha256 = Some(digest('c'));
    record.availability = CatalogAvailability::Available;
    record.validate().unwrap();
    let provenance = VerifiedCatalogProvenance {
        registry_name: "packages".to_owned(),
        registry_url: "https://packages.example.test/a3s/".to_owned(),
        root_sha256: digest('f'),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: record.descriptor_digest().unwrap(),
    };
    let verified = VerifiedPluginCatalogRecord::new(record, provenance).unwrap();
    PluginPackageResolver::new(
        PluginPackageLockHost::new("linux-x86_64", env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .resolve(verified, Vec::new())
    .unwrap()
}

fn package_selections(
    package_lock: &PluginPackageLock,
    generation: u64,
) -> Vec<InstallationPackageSelection> {
    package_lock
        .packages
        .iter()
        .cloned()
        .map(|package| {
            let selected_surfaces = package
                .catalog
                .record
                .resolve_surfaces(&[])
                .unwrap()
                .into_iter()
                .map(|surface| surface.reference())
                .collect();
            InstallationPackageSelection::new(package, generation, true, selected_surfaces).unwrap()
        })
        .collect()
}

async fn write_receipt_fixture(paths: &ExtensionPaths, digest: &str, retained: bool) {
    let raw_digest = digest.strip_prefix("sha256:").unwrap();
    let receipt = ExtensionReceipt {
        schema_version: EXTENSION_RECEIPT_SCHEMA_VERSION,
        installation: paths.installation().clone(),
        package_id: "acme/knowledge".to_owned(),
        component_id: "use/acme/knowledge".to_owned(),
        route_alias: Some("knowledge".to_owned()),
        version: "1.0.0".to_owned(),
        package_root: paths
            .artifact_store()
            .expanded_package_path(digest)
            .unwrap(),
        manifest_sha256: "c".repeat(64),
        package_sha256: Some(raw_digest.to_owned()),
        trust: ExtensionTrust::LocalExplicit,
        registry: None,
        verified_catalog: None,
        planning_bundle: None,
        selected_surfaces: Vec::new(),
        installed_at_unix: 1,
        enabled: true,
        lifecycle_generation: Some(7),
    };
    let current_root = paths.installation_state_root().join("extensions");
    fs::create_dir_all(&current_root).await.unwrap();
    fs::write(current_root.join(".registry.lock"), b"")
        .await
        .unwrap();
    let path = if retained {
        paths
            .installation_state_root()
            .join("extension-generations/acme/knowledge")
            .join(format!("{:020}-{raw_digest}.json", 7))
    } else {
        current_root.join("acme/knowledge.json")
    };
    fs::create_dir_all(path.parent().unwrap()).await.unwrap();
    fs::write(path, serde_json::to_vec_pretty(&receipt).unwrap())
        .await
        .unwrap();
}

fn contains_reference(
    inventory: &ArtifactReferenceInventory,
    source: ArtifactReferenceSource,
    installation: &InstallationId,
    digest: &str,
) -> bool {
    inventory.entries.iter().any(|entry| {
        entry.kind == ArtifactKind::ExpandedPackage
            && entry.digest == digest
            && entry.source == source
            && entry.installation.as_ref() == Some(installation)
    })
}
