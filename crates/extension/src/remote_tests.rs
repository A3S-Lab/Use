use std::path::{Path, PathBuf};

use a3s_use_core::{
    CatalogPlanningTarget, ExecutablePlanningSurface, PlanningArtifactRef,
    PlanningSurfaceActivation, PluginCatalogRecord, PluginPlanningBundle, PluginSurfaceKind,
    ToolReleaseDescriptor, PLUGIN_CATALOG_SCHEMA_V3, PLUGIN_PLANNING_BUNDLE_SCHEMA,
};
use sha2::{Digest, Sha256};

use super::test_support::{
    extension_archive, find_subslice, TestRepository, TestServer, TestTarget, EXPIRED, FUTURE,
    PACKAGE_VERSION,
};
use super::*;

const COMPLETE_CATALOG: &[u8] =
    include_bytes!("../../core/fixtures/plugins/complete-package-catalog-v3.json");

#[tokio::test]
async fn tuf_refresh_verifies_metadata_without_downloading_targets() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let metadata = refresh_remote_registry(&trusted).await.unwrap();

    assert_eq!(metadata.registry_name, "fixture");
    assert_eq!(metadata.root_version, 1);
    assert_eq!(metadata.timestamp_version, 7);
    assert_eq!(metadata.snapshot_version, 7);
    assert_eq!(metadata.targets_version, 7);
    assert_eq!(metadata.package_targets, 1);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn caller_pinned_root_is_idempotent_and_never_downloaded() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let root = repository
        .routes
        .get("/metadata/root.json")
        .expect("root route")
        .clone();
    let mut routes = repository.routes.clone();
    routes.remove("/metadata/root.json");
    let server = TestServer::start(routes);
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());

    let inspected = inspect_bootstrap_root(&root).unwrap();
    let admitted = trusted.pin_trusted_root(&root).await.unwrap();
    let replayed = trusted.pin_trusted_root(&root).await.unwrap();
    assert_eq!(admitted, inspected);
    assert_eq!(admitted, replayed);
    assert_eq!(admitted.root_sha256, repository.root_sha256);
    assert_eq!(admitted.root_version, 1);
    assert_eq!(admitted.size_bytes, root.len() as u64);
    let mut replacement = b"\n".to_vec();
    replacement.extend_from_slice(&root);
    let replacement_registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        format!("{:x}", Sha256::digest(&replacement)),
        None,
        datastore,
        ArtifactStore::from_data_root(&temp.path().join("data")),
    )
    .unwrap();
    let error = replacement_registry
        .pin_trusted_root(&replacement)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_root_conflict");
    let metadata = refresh_remote_registry(&trusted).await.unwrap();

    assert_eq!(metadata.root_version, 1);
    assert!(server
        .requests()
        .iter()
        .all(|request| request != "/metadata/root.json"));
}

#[test]
fn bootstrap_root_inspection_derives_exact_evidence_without_registry_state() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let root = repository
        .routes
        .get("/metadata/root.json")
        .expect("root route");

    let inspected = inspect_bootstrap_root(root).expect("bootstrap root evidence");

    assert_eq!(inspected.root_sha256, repository.root_sha256);
    assert_eq!(inspected.root_version, 1);
    assert_eq!(inspected.size_bytes, root.len() as u64);
    for invalid in [Vec::new(), b"{\"signed\":{}}".to_vec()] {
        let error = inspect_bootstrap_root(&invalid).expect_err("invalid bootstrap root");
        assert_eq!(error.code, "use.extension.registry_root_invalid");
    }
    let oversized = vec![0_u8; MAX_BOOTSTRAP_ROOT_BYTES as usize + 1];
    let error = inspect_bootstrap_root(&oversized).expect_err("oversized bootstrap root");
    assert_eq!(error.code, "use.extension.registry_root_invalid");
}

#[tokio::test]
async fn caller_pinned_root_rejects_empty_oversized_and_mismatched_bytes() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    for bytes in [Vec::new(), b"different root".to_vec()] {
        assert!(trusted.pin_trusted_root(&bytes).await.is_err());
    }
    let oversized = vec![0_u8; MAX_BOOTSTRAP_ROOT_BYTES as usize + 1];
    let error = trusted.pin_trusted_root(&oversized).await.unwrap_err();

    assert_eq!(error.code, "use.extension.registry_root_invalid");
    let malformed = b"{\"signed\":{}}";
    let malformed_registry = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        format!("{:x}", Sha256::digest(malformed)),
        None,
        temp.path().join("malformed-tuf"),
        ArtifactStore::from_data_root(&temp.path().join("data")),
    )
    .unwrap();
    let error = malformed_registry
        .pin_trusted_root(malformed)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_root_invalid");
    assert!(!malformed_registry.datastore().exists());
    let root = repository
        .routes
        .get("/metadata/root.json")
        .expect("root route");
    tokio::fs::create_dir_all(temp.path().join("tuf").join(ROOT_CACHE_NAME))
        .await
        .unwrap();
    let error = trusted.pin_trusted_root(root).await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_path_invalid");

    let explicit = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        Some(temp.path().join("explicit-root.json")),
        temp.path().join("explicit-tuf"),
        ArtifactStore::from_data_root(&temp.path().join("data")),
    )
    .unwrap();
    let error = explicit.pin_trusted_root(root).await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_path_invalid");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn tuf_catalog_lists_signed_packages_without_downloading_targets() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let catalog = list_remote_packages(&trusted).await.unwrap();

    assert_eq!(catalog.metadata.registry_name, "fixture");
    assert_eq!(catalog.metadata.package_targets, 1);
    assert_eq!(catalog.packages.len(), 1);
    assert_eq!(catalog.packages[0].package_id, "a3s/science");
    assert_eq!(catalog.packages[0].version, PACKAGE_VERSION);
    assert_eq!(catalog.packages[0].target, catalog.host_target);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn catalog_v3_loads_only_the_exact_signed_planning_target() {
    let (repository, expected, archive_target, planning_target) = planning_test_repository(false);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();
    let actual = prepared.load_planning_bundle().await.unwrap().unwrap();

    assert_eq!(actual, expected);
    assert!(server
        .requests()
        .iter()
        .any(|request| request == &format!("/targets/{planning_target}")));
    assert!(server
        .requests()
        .iter()
        .all(|request| request != &format!("/targets/{archive_target}")));
}

#[tokio::test]
async fn executable_archive_download_retains_its_verified_planning_bundle() {
    let (repository, expected, archive_target, planning_target) = planning_test_repository(false);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();

    let downloaded = prepared.download().await.unwrap();

    assert_eq!(downloaded.planning_bundle(), Some(&expected));
    assert!(server
        .requests()
        .iter()
        .any(|request| request == &format!("/targets/{planning_target}")));
    assert!(server
        .requests()
        .iter()
        .any(|request| request == &format!("/targets/{archive_target}")));
}

#[tokio::test]
async fn verified_targets_are_content_addressed_and_reusable_without_network() {
    let (repository, expected, _, _) = planning_test_repository(false);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());

    refresh_remote_registry(&trusted).await.unwrap();
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let archive_sha256 = prepared.resolved().sha256.clone();
    let planning_sha256 = prepared
        .verified_catalog()
        .record
        .planning
        .as_ref()
        .unwrap()
        .sha256
        .trim_start_matches("sha256:")
        .to_owned();
    let online = prepared.download().await.unwrap();
    let online_archive = std::fs::read(online.path()).unwrap();

    for digest in [&archive_sha256, &planning_sha256] {
        let path = target_observation_path(&datastore, digest);
        assert!(
            path.is_file(),
            "missing Registry target observation {}",
            path.display()
        );
        let blob = global_blob_path(&trusted, digest);
        assert!(blob.is_file(), "missing global blob {}", blob.display());
    }

    server.clear_requests();
    let cached = prepare_cached_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let cached = cached.download().await.unwrap();

    assert_eq!(cached.planning_bundle(), Some(&expected));
    assert_eq!(std::fs::read(cached.path()).unwrap(), online_archive);
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn registry_sources_share_global_blobs_without_sharing_observations() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive.clone(), 13, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let artifact_store = ArtifactStore::from_data_root(&temp.path().join("data"));
    let first = TrustedRegistry::new(
        "first",
        server.base_url(),
        &repository.root_sha256,
        None,
        temp.path().join("first-tuf"),
        artifact_store.clone(),
    )
    .unwrap();
    let second = TrustedRegistry::new(
        "second",
        server.base_url(),
        &repository.root_sha256,
        None,
        temp.path().join("second-tuf"),
        artifact_store,
    )
    .unwrap();
    refresh_remote_registry(&first).await.unwrap();
    refresh_remote_registry(&second).await.unwrap();
    let first_package = prepare_remote_package(&first, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = first_package.resolved().sha256.clone();
    let downloaded = first_package.download().await.unwrap();
    assert_eq!(std::fs::read(downloaded.path()).unwrap(), archive);
    assert!(target_observation_path(first.datastore(), &digest).is_file());
    assert!(!target_observation_path(second.datastore(), &digest).exists());

    server.clear_requests();
    let second_package =
        prepare_cached_remote_package(&second, "a3s/science", None, "stable", None)
            .await
            .unwrap();
    let downloaded = second_package.download().await.unwrap();

    assert_eq!(std::fs::read(downloaded.path()).unwrap(), archive);
    assert!(target_observation_path(second.datastore(), &digest).is_file());
    assert!(global_blob_path(&second, &digest).is_file());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn missing_global_blob_does_not_mutate_source_cache_during_cached_staging() {
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let cache = datastore.join("verified-targets/sha256");
    tokio::fs::create_dir_all(&cache).await.unwrap();
    let artifact_store = ArtifactStore::from_data_root(&temp.path().join("data"));
    let registry = TrustedRegistry::new(
        "fixture",
        "https://registry.example.test/",
        "c".repeat(64),
        None,
        datastore.clone(),
        artifact_store.clone(),
    )
    .unwrap()
    .with_target_cache_policy(VerifiedTargetCachePolicy::new(3, 1, 0).unwrap());
    let retained = b"old";
    let retained_digest = format!("{:x}", Sha256::digest(retained));
    let source_path = temp.path().join("retained.part");
    tokio::fs::write(&source_path, retained).await.unwrap();
    let mut source = tokio::fs::File::open(&source_path).await.unwrap();
    let artifact_admission = artifact_store.acquire_reference_admission().await.unwrap();
    drop(
        artifact_store
            .commit_blob(
                &artifact_admission,
                &mut source,
                retained.len() as u64,
                &retained_digest,
            )
            .await
            .unwrap(),
    );
    super::target_cache::record::write_observation(&cache, &retained_digest, retained.len() as u64)
        .await
        .unwrap();

    let error =
        super::target_cache::stage_cached_target(&registry, "missing.bin", 3, &"b".repeat(64))
            .await
            .unwrap_err();

    assert_eq!(error.code, "use.extension.registry_target_cache_missing");
    assert!(target_observation_path(&datastore, &retained_digest).is_file());
    assert!(global_blob_path(&registry, &retained_digest).is_file());
}

#[tokio::test]
async fn cached_target_tampering_and_missing_content_fail_closed_without_network() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive, 17, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());

    refresh_remote_registry(&trusted).await.unwrap();
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().sha256.clone();
    prepared.download().await.unwrap();
    let blob_path = global_blob_path(&trusted, &digest);

    std::fs::write(&blob_path, b"tampered cache bytes").unwrap();
    server.clear_requests();
    let prepared = prepare_cached_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let error = prepared.download().await.unwrap_err();
    assert_eq!(error.code, "use.artifact_store.blob_invalid");
    assert!(server.requests().is_empty());

    std::fs::remove_file(&blob_path).unwrap();
    let prepared = prepare_cached_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let error = prepared.download().await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_target_cache_missing");
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn source_entry_retention_never_deletes_global_blobs() {
    let (repository, _, _, _) = planning_test_repository(false);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let policy =
        VerifiedTargetCachePolicy::new(DEFAULT_VERIFIED_TARGET_CACHE_MAX_BYTES, 1, 0).unwrap();
    let trusted =
        trusted_registry(&server, &repository, datastore.clone()).with_target_cache_policy(policy);
    refresh_remote_registry(&trusted).await.unwrap();
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let archive_digest = prepared.resolved().sha256.clone();
    let planning_digest = prepared
        .verified_catalog()
        .record
        .planning
        .as_ref()
        .unwrap()
        .sha256
        .trim_start_matches("sha256:")
        .to_owned();

    let downloaded = prepared.download().await.unwrap();

    assert!(downloaded.planning_bundle().is_some());
    let usage = inspect_verified_target_cache(&trusted).await.unwrap();
    assert_eq!(usage.target_entries, 1);
    assert!(target_observation_path(&datastore, &archive_digest).is_file());
    assert!(!target_observation_path(&datastore, &planning_digest).exists());
    assert!(global_blob_path(&trusted, &archive_digest).is_file());
    assert!(global_blob_path(&trusted, &planning_digest).is_file());

    server.clear_requests();
    let cached = prepare_cached_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let cached = cached.download().await.unwrap();
    assert!(cached.planning_bundle().is_some());
    assert!(server.requests().is_empty());
}

#[tokio::test]
async fn catalog_v3_static_package_has_no_planning_target_download() {
    let archive = extension_archive(PACKAGE_VERSION);
    let target = host_target().unwrap();
    let archive_target = format!(
        "extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/science-fixture-{PACKAGE_VERSION}-{target}.tar.gz"
    );
    let mut catalog = PluginCatalogRecord::from_json(COMPLETE_CATALOG).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    catalog.package_id = "a3s/science".to_owned();
    catalog.display_name = "A3S Science".to_owned();
    catalog.description = "Static scientific guidance for A3S agents.".to_owned();
    catalog.publisher = "a3s".to_owned();
    catalog.version = PACKAGE_VERSION.to_owned();
    catalog.requires_use = ">=0.3.0, <0.4.0".to_owned();
    catalog.target = target;
    catalog
        .surfaces
        .retain(|surface| surface.kind == PluginSurfaceKind::Skill && surface.id == "review");
    catalog.permission_ceiling.surfaces.clear();
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    catalog.planning = None;
    catalog.archive.target_name = archive_target.clone();
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.manifest_sha256 = Some(format!("sha256:{}", "c".repeat(64)));
    catalog.repository = "https://github.com/A3S-Lab/Science".to_owned();
    catalog.validate().unwrap();
    let repository = TestRepository::with_target_metadata(
        archive,
        archive_target,
        serde_json::to_value(catalog).unwrap(),
        13,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();
    assert!(prepared.load_planning_bundle().await.unwrap().is_none());
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn catalog_v3_rejects_planning_target_metadata_drift_before_download() {
    let (repository, _, archive_target, planning_target) = planning_test_repository(true);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    server.clear_requests();
    let error = prepared.load_planning_bundle().await.unwrap_err();

    assert_eq!(error.code, "use.extension.registry_planning_target_invalid");
    assert!(server.requests().iter().all(|request| {
        request != &format!("/targets/{planning_target}")
            && request != &format!("/targets/{archive_target}")
    }));
}

#[tokio::test]
async fn reviewed_registry_plan_fails_before_target_download() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let error = prepare_remote_package(
        &trusted,
        "a3s/science",
        None,
        "stable",
        Some(&"0".repeat(64)),
    )
    .await
    .unwrap_err();

    assert_eq!(error.code, "use.extension.registry_plan_mismatch");
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_rejects_wrong_root_and_tampered_target() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive, 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let wrong = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        "f".repeat(64),
        None,
        temp.path().join("wrong-root"),
        ArtifactStore::from_data_root(&temp.path().join("data")),
    )
    .unwrap();
    let error = prepare_remote_package(&wrong, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_root_mismatch");

    let mut routes = repository.routes.clone();
    routes.insert(
        format!("/targets/{}", repository.target_name),
        b"tampered archive".to_vec(),
    );
    let tampered_server = TestServer::start(routes);
    let trusted = trusted_registry(
        &tampered_server,
        &repository,
        temp.path().join("tampered-target"),
    );
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let error = prepared.download().await.unwrap_err();
    assert_eq!(error.code, "use.extension.registry_download_failed");
}

#[tokio::test]
async fn tuf_rejects_metadata_tampering_expiration_and_rollback() {
    let archive = extension_archive(PACKAGE_VERSION);
    let version_two = TestRepository::new(archive.clone(), 2, FUTURE);
    let server_two = TestServer::start(version_two.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("rollback-state");
    let trusted_two = trusted_registry(&server_two, &version_two, datastore.clone());
    prepare_remote_package(&trusted_two, "a3s/science", None, "stable", None)
        .await
        .unwrap();

    let version_one = TestRepository::new(archive.clone(), 1, FUTURE);
    assert_eq!(version_one.root_sha256, version_two.root_sha256);
    let server_one = TestServer::start(version_one.routes.clone());
    let trusted_one = trusted_registry(&server_one, &version_one, datastore);
    let rollback = prepare_remote_package(&trusted_one, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(rollback.code, "use.extension.registry_untrusted");

    let expired = TestRepository::new(archive.clone(), 1, EXPIRED);
    let expired_server = TestServer::start(expired.routes.clone());
    let expired_registry =
        trusted_registry(&expired_server, &expired, temp.path().join("expired-state"));
    let error = prepare_remote_package(&expired_registry, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_untrusted");

    let mut tampered_routes = version_one.routes.clone();
    let targets = tampered_routes.get_mut("/metadata/targets.json").unwrap();
    let position = find_subslice(targets, b"stable").unwrap();
    targets[position..position + 6].copy_from_slice(b"nightl");
    let tampered_server = TestServer::start(tampered_routes);
    let tampered_registry = trusted_registry(
        &tampered_server,
        &version_one,
        temp.path().join("tampered-metadata"),
    );
    let error = prepare_remote_package(&tampered_registry, "a3s/science", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_untrusted");
}

fn trusted_registry(
    server: &TestServer,
    repository: &TestRepository,
    datastore: PathBuf,
) -> TrustedRegistry {
    let artifact_store = ArtifactStore::from_data_root(
        &datastore
            .parent()
            .unwrap_or(datastore.as_path())
            .join("data"),
    );
    TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        datastore,
        artifact_store,
    )
    .unwrap()
}

fn target_observation_path(datastore: &Path, digest: &str) -> PathBuf {
    datastore
        .join("verified-targets/sha256")
        .join(format!("{digest}.json"))
}

fn global_blob_path(registry: &TrustedRegistry, digest: &str) -> PathBuf {
    registry
        .artifact_store()
        .blob_path(&format!("sha256:{digest}"))
        .unwrap()
}

fn planning_test_repository(
    mismatched_catalog_digest: bool,
) -> (TestRepository, PluginPlanningBundle, String, String) {
    let archive = extension_archive(PACKAGE_VERSION);
    let target = host_target().unwrap();
    let archive_target = format!(
        "extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/science-fixture-{PACKAGE_VERSION}-{target}.tar.gz"
    );
    let planning_target =
        format!("extensions/a3s/science/{PACKAGE_VERSION}/stable/{target}/planning-v1.json");
    let mut catalog = PluginCatalogRecord::from_json(COMPLETE_CATALOG).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_owned();
    catalog.package_id = "a3s/science".to_owned();
    catalog.display_name = "A3S Science".to_owned();
    catalog.description = "Scientific research capabilities for A3S agents.".to_owned();
    catalog.publisher = "a3s".to_owned();
    catalog.version = PACKAGE_VERSION.to_owned();
    catalog.requires_use = ">=0.3.0, <0.4.0".to_owned();
    catalog.target = target;
    catalog.archive.target_name = archive_target.clone();
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.manifest_sha256 = Some(format!("sha256:{}", "c".repeat(64)));
    catalog.repository = "https://github.com/A3S-Lab/Science".to_owned();
    catalog.surfaces = vec![catalog
        .surfaces
        .iter()
        .find(|surface| surface.kind == PluginSurfaceKind::Tool && surface.id == "index")
        .unwrap()
        .clone()];
    catalog.permission_ceiling.surfaces = vec![catalog
        .permission_ceiling
        .surfaces
        .iter()
        .find(|permission| {
            permission.surface.kind == PluginSurfaceKind::Tool && permission.surface.id == "index"
        })
        .unwrap()
        .clone()];
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();

    let descriptor = ToolReleaseDescriptor::from_json(include_bytes!(
        "../../core/fixtures/releases/tool-service-release-v1.json"
    ))
    .unwrap();
    let bundle = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: catalog.package_id.clone(),
        version: catalog.version.clone(),
        channel: catalog.channel,
        target: catalog.target.clone(),
        archive_sha256: catalog.archive.sha256.clone(),
        package_sha256: catalog.package.sha256.clone().unwrap(),
        manifest_sha256: catalog.package.manifest_sha256.clone().unwrap(),
        permission_ceiling_digest: catalog.permission_ceiling_digest.clone(),
        surfaces: vec![ExecutablePlanningSurface::ToolService {
            id: "index".to_owned(),
            activation: PlanningSurfaceActivation::Eager,
            base_path: "/api".to_owned(),
            artifact: PlanningArtifactRef {
                uri: format!(
                    "oci://registry.example/a3s/science-index@{}",
                    descriptor.artifact.digest
                ),
                digest: descriptor.artifact.digest.clone(),
                media_type: descriptor.artifact.media_type.clone(),
            },
            descriptor,
        }],
    };
    let planning_bytes = bundle.canonical_bytes().unwrap();
    catalog.planning = Some(CatalogPlanningTarget {
        target_name: planning_target.clone(),
        length: planning_bytes.len() as u64,
        sha256: if mismatched_catalog_digest {
            format!("sha256:{}", "e".repeat(64))
        } else {
            format!("sha256:{:x}", Sha256::digest(&planning_bytes))
        },
    });
    catalog.validate().unwrap();

    let repository = TestRepository::with_targets(
        vec![
            TestTarget {
                archive,
                target_name: archive_target.clone(),
                custom: Some(serde_json::to_value(catalog).unwrap()),
            },
            TestTarget {
                archive: planning_bytes,
                target_name: planning_target.clone(),
                custom: None,
            },
        ],
        11,
        FUTURE,
    );
    (repository, bundle, archive_target, planning_target)
}
