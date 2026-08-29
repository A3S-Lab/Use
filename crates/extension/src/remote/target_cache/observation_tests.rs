use super::*;
use crate::ArtifactStore;

fn cache_directory(datastore: &Path) -> PathBuf {
    let cache = datastore.join("verified-targets/sha256");
    std::fs::create_dir_all(&cache).unwrap();
    cache
}

#[tokio::test]
async fn exact_target_observation_reports_missing_partial_and_complete_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let datastore = &temporary.path().join("state");
    let cache = cache_directory(datastore);
    let artifact_store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let body = b"12345678";
    let digest = format!("{:x}", Sha256::digest(body));

    let missing = observe_target_cache_entry(datastore, &artifact_store, "fixture", 8, &digest)
        .await
        .unwrap();
    assert_eq!(missing.status, VerifiedTargetObservationStatus::Missing);
    assert_eq!(missing.retained_bytes, 0);
    assert_eq!(missing.expected_bytes, 8);
    assert_eq!(missing.target_digest, format!("sha256:{digest}"));

    let partial_path = cache.join(format!(".target-{digest}.part"));
    std::fs::write(&partial_path, b"abc").unwrap();
    let partial = observe_target_cache_entry(datastore, &artifact_store, "fixture", 8, &digest)
        .await
        .unwrap();
    assert_eq!(partial.status, VerifiedTargetObservationStatus::Partial);
    assert_eq!(partial.retained_bytes, 3);

    std::fs::remove_file(partial_path).unwrap();
    let source = temporary.path().join("target.part");
    fs::write(&source, body).await.unwrap();
    let mut source = fs::File::open(source).await.unwrap();
    let artifact_admission = artifact_store.acquire_reference_admission().await.unwrap();
    artifact_store
        .commit_blob(&artifact_admission, &mut source, body.len() as u64, &digest)
        .await
        .unwrap();
    record::write_observation(&cache, &digest, body.len() as u64)
        .await
        .unwrap();
    let complete = observe_target_cache_entry(datastore, &artifact_store, "fixture", 8, &digest)
        .await
        .unwrap();
    assert_eq!(complete.status, VerifiedTargetObservationStatus::Complete);
    assert_eq!(complete.retained_bytes, 8);
}

#[tokio::test]
async fn exact_target_observation_fails_closed_on_dangling_or_oversized_state() {
    let temporary = tempfile::tempdir().unwrap();
    let datastore = &temporary.path().join("state");
    let cache = cache_directory(datastore);
    let artifact_store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = "b".repeat(64);
    record::write_observation(&cache, &digest, 4).await.unwrap();

    let dangling = observe_target_cache_entry(datastore, &artifact_store, "fixture", 4, &digest)
        .await
        .unwrap_err();
    assert_eq!(dangling.code, "use.extension.registry_target_cache_invalid");

    std::fs::remove_file(record::observation_path(&cache, &digest)).unwrap();
    std::fs::write(cache.join(format!(".target-{digest}.part")), b"12345").unwrap();
    let oversized = observe_target_cache_entry(datastore, &artifact_store, "fixture", 4, &digest)
        .await
        .unwrap_err();
    assert_eq!(
        oversized.code,
        "use.extension.registry_target_cache_invalid"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn exact_target_observation_rejects_links_without_following_them() {
    let temporary = tempfile::tempdir().unwrap();
    let datastore = &temporary.path().join("state");
    let cache = cache_directory(datastore);
    let artifact_store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = "c".repeat(64);
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"secret").unwrap();
    crate::test_filesystem::create_directory_link(
        &outside,
        &cache.join(format!(".target-{digest}.part")),
    );

    let error = observe_target_cache_entry(datastore, &artifact_store, "fixture", 8, &digest)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
    assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"secret");
}
