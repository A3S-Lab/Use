use std::path::PathBuf;

use super::test_support::{
    extension_archive, find_subslice, TestRepository, TestServer, EXPIRED, FUTURE, PACKAGE_VERSION,
};
use super::*;
use crate::{ExtensionPaths, ExtensionRegistry, ExtensionTrust};

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
async fn tuf_catalog_lists_signed_packages_without_downloading_targets() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let catalog = list_remote_packages(&trusted).await.unwrap();

    assert_eq!(catalog.metadata.registry_name, "fixture");
    assert_eq!(catalog.metadata.package_targets, 1);
    assert_eq!(catalog.packages.len(), 1);
    assert_eq!(catalog.packages[0].package_id, "acme/slack");
    assert_eq!(catalog.packages[0].version, PACKAGE_VERSION);
    assert_eq!(catalog.packages[0].target, catalog.host_target);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_install_records_signed_provenance_and_converges() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive, 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let prepared = prepare_remote_package(&trusted, "acme/slack", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().plan_digest().unwrap();
    drop(prepared);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));

    let paths = ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("extension-state"),
    );
    let registry = ExtensionRegistry::new(paths);
    let installed = registry
        .install_remote("acme/slack", &trusted, None, "stable", Some(&digest), false)
        .await
        .unwrap();
    assert!(installed.changed);
    assert_eq!(
        installed.extension.receipt.trust,
        ExtensionTrust::RegistryTuf
    );
    let provenance = installed.extension.receipt.registry.as_ref().unwrap();
    assert_eq!(provenance.package_id, "acme/slack");
    assert_eq!(provenance.version, PACKAGE_VERSION);
    assert_eq!(provenance.sha256, repository.target_sha256);
    assert!(installed.extension.cli_executable().unwrap().is_file());

    server.clear_requests();
    let second = registry
        .install_remote("acme/slack", &trusted, None, "stable", Some(&digest), false)
        .await
        .unwrap();
    assert!(!second.changed);
    assert_eq!(registry.list().await.unwrap().len(), 1);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_convergence_refreshes_signed_provenance_without_downloading_the_target() {
    let archive = extension_archive(PACKAGE_VERSION);
    let first_repository = TestRepository::new(archive.clone(), 1, FUTURE);
    let server = TestServer::start(first_repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &first_repository, temp.path().join("tuf"));
    let paths = ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("extension-state"),
    );
    let registry = ExtensionRegistry::new(paths);
    registry
        .install_remote("acme/slack", &trusted, None, "stable", None, false)
        .await
        .unwrap();

    let second_repository = TestRepository::new(archive, 2, FUTURE);
    assert_eq!(
        second_repository.target_sha256,
        first_repository.target_sha256
    );
    server.replace_routes(second_repository.routes);
    server.clear_requests();

    let converged = registry
        .install_remote("acme/slack", &trusted, None, "stable", None, false)
        .await
        .unwrap();

    assert!(!converged.changed);
    let provenance = converged.extension.receipt.registry.unwrap();
    assert_eq!(provenance.timestamp_version, 2);
    assert_eq!(provenance.snapshot_version, 2);
    assert_eq!(provenance.targets_version, 2);
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_install_rejects_modified_installed_content_before_dispatch_or_convergence() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let paths = ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("extension-state"),
    );
    let registry = ExtensionRegistry::new(paths);
    let installed = registry
        .install_remote("acme/slack", &trusted, None, "stable", None, false)
        .await
        .unwrap();
    std::fs::write(
        installed.extension.cli_executable().unwrap(),
        b"modified executable",
    )
    .unwrap();

    let dispatch_error = match registry.acquire_route("slack").await {
        Err(error) => error,
        Ok(_) => panic!("modified signed content must not be dispatched"),
    };
    assert_eq!(dispatch_error.code, "use.extension.package_digest_mismatch");

    server.clear_requests();
    let convergence_error = registry
        .install_remote("acme/slack", &trusted, None, "stable", None, false)
        .await
        .unwrap_err();
    assert_eq!(
        convergence_error.code,
        "use.extension.package_digest_mismatch"
    );
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[tokio::test]
async fn tuf_receipt_requires_an_expanded_package_digest() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));
    let paths = ExtensionPaths::new(
        temp.path().join("data"),
        temp.path().join("extension-state"),
    );
    let registry = ExtensionRegistry::new(paths);
    registry
        .install_remote("acme/slack", &trusted, None, "stable", None, false)
        .await
        .unwrap();

    let receipt_path = registry.paths().receipt_path("acme/slack");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    receipt.as_object_mut().unwrap().remove("packageSha256");
    std::fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap()).unwrap();

    let error = registry.get("acme/slack").await.unwrap_err();
    assert_eq!(error.code, "use.extension.receipt_invalid");
}

#[tokio::test]
async fn reviewed_registry_plan_fails_before_target_download() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = trusted_registry(&server, &repository, temp.path().join("tuf"));

    let error = prepare_remote_package(
        &trusted,
        "acme/slack",
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
    )
    .unwrap();
    let error = prepare_remote_package(&wrong, "acme/slack", None, "stable", None)
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
    let prepared = prepare_remote_package(&trusted, "acme/slack", None, "stable", None)
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
    prepare_remote_package(&trusted_two, "acme/slack", None, "stable", None)
        .await
        .unwrap();

    let version_one = TestRepository::new(archive.clone(), 1, FUTURE);
    assert_eq!(version_one.root_sha256, version_two.root_sha256);
    let server_one = TestServer::start(version_one.routes.clone());
    let trusted_one = trusted_registry(&server_one, &version_one, datastore);
    let rollback = prepare_remote_package(&trusted_one, "acme/slack", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(rollback.code, "use.extension.registry_untrusted");

    let expired = TestRepository::new(archive.clone(), 1, EXPIRED);
    let expired_server = TestServer::start(expired.routes.clone());
    let expired_registry =
        trusted_registry(&expired_server, &expired, temp.path().join("expired-state"));
    let error = prepare_remote_package(&expired_registry, "acme/slack", None, "stable", None)
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
    let error = prepare_remote_package(&tampered_registry, "acme/slack", None, "stable", None)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_untrusted");
}

fn trusted_registry(
    server: &TestServer,
    repository: &TestRepository,
    datastore: PathBuf,
) -> TrustedRegistry {
    TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        datastore,
    )
    .unwrap()
}
