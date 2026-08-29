use std::path::{Path, PathBuf};

use super::test_support::{extension_archive, TestRepository, TestServer, FUTURE, PACKAGE_VERSION};
use super::*;

#[tokio::test]
async fn interrupted_target_download_resumes_from_a_durable_verified_prefix() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive.clone(), 19, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());
    let target_path = format!("/targets/{}", repository.target_name);
    let chunk_bytes = (archive.len() / 8).max(1);

    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().sha256.clone();
    server.clear_requests();
    server.interrupt_requests(&target_path, 20, chunk_bytes);

    let error = prepared.download().await.unwrap_err();

    assert_eq!(error.code, "use.extension.registry_download_failed");
    let partial = datastore
        .join("verified-targets/sha256")
        .join(format!(".target-{digest}.part"));
    let partial_length = std::fs::metadata(&partial).unwrap().len();
    assert!(partial_length > 0);
    assert!(partial_length < archive.len() as u64);

    server.allow_complete_requests(&target_path);
    server.clear_requests();
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let downloaded = prepared.download().await.unwrap();

    assert_eq!(std::fs::read(downloaded.path()).unwrap(), archive);
    assert_eq!(
        server.ranges_for(&target_path),
        vec![format!("bytes={partial_length}-")]
    );
    assert!(!partial.exists());
    assert!(target_observation_path(&datastore, &digest).is_file());
    assert!(global_blob_path(&trusted, &digest).is_file());
}

#[tokio::test]
async fn tampered_resumable_prefix_fails_digest_verification_and_is_discarded() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive.clone(), 23, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());
    let target_path = format!("/targets/{}", repository.target_name);
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().sha256.clone();
    let partial = datastore
        .join("verified-targets/sha256")
        .join(format!(".target-{digest}.part"));
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    let partial_length = (archive.len() / 3).max(1);
    let mut tampered = archive[..partial_length].to_vec();
    tampered[0] ^= 0xff;
    std::fs::write(&partial, tampered).unwrap();
    server.clear_requests();

    let error = prepared.download().await.unwrap_err();

    assert_eq!(error.code, "use.extension.registry_download_failed");
    assert_eq!(
        server.ranges_for(&target_path),
        vec![format!("bytes={partial_length}-")]
    );
    assert!(!partial.exists());
    assert!(!target_observation_path(&datastore, &digest).exists());
    assert!(!global_blob_path(&trusted, &digest).exists());

    server.clear_requests();
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let downloaded = prepared.download().await.unwrap();
    assert_eq!(std::fs::read(downloaded.path()).unwrap(), archive);
    assert!(server.ranges_for(&target_path).is_empty());
}

#[tokio::test]
async fn mismatched_content_range_discards_the_unverified_partial() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive.clone(), 29, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());
    let target_path = format!("/targets/{}", repository.target_name);
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().sha256.clone();
    let partial = datastore
        .join("verified-targets/sha256")
        .join(format!(".target-{digest}.part"));
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    let partial_length = (archive.len() / 4).max(1);
    std::fs::write(&partial, &archive[..partial_length]).unwrap();
    server.override_content_range(&target_path, "bytes 0-0/1");
    server.clear_requests();

    let error = prepared.download().await.unwrap_err();

    assert_eq!(error.code, "use.extension.registry_download_failed");
    assert!(error.message.contains("range response"));
    assert_eq!(
        server.ranges_for(&target_path),
        vec![format!("bytes={partial_length}-")]
    );
    assert!(!partial.exists());
    assert!(!target_observation_path(&datastore, &digest).exists());
    assert!(!global_blob_path(&trusted, &digest).exists());
}

#[tokio::test]
async fn server_that_ignores_range_restarts_from_a_complete_response() {
    let archive = extension_archive(PACKAGE_VERSION);
    let repository = TestRepository::new(archive.clone(), 31, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let datastore = temp.path().join("tuf");
    let trusted = trusted_registry(&server, &repository, datastore.clone());
    let target_path = format!("/targets/{}", repository.target_name);
    let prepared = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let digest = prepared.resolved().sha256.clone();
    let partial = datastore
        .join("verified-targets/sha256")
        .join(format!(".target-{digest}.part"));
    std::fs::create_dir_all(partial.parent().unwrap()).unwrap();
    let partial_length = (archive.len() / 5).max(1);
    std::fs::write(&partial, &archive[..partial_length]).unwrap();
    server.ignore_ranges_for(&target_path);
    server.clear_requests();

    let downloaded = prepared.download().await.unwrap();

    assert_eq!(std::fs::read(downloaded.path()).unwrap(), archive);
    assert_eq!(
        server.ranges_for(&target_path),
        vec![format!("bytes={partial_length}-")]
    );
    assert!(!partial.exists());
    assert!(target_observation_path(&datastore, &digest).is_file());
    assert!(global_blob_path(&trusted, &digest).is_file());
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
