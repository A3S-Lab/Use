use super::*;

fn cache_directory(datastore: &Path) -> PathBuf {
    let cache = datastore.join("verified-targets/sha256");
    std::fs::create_dir_all(&cache).unwrap();
    cache
}

#[tokio::test]
async fn exact_target_observation_reports_missing_partial_and_complete_bytes() {
    let temporary = tempfile::tempdir().unwrap();
    let datastore = temporary.path();
    let cache = cache_directory(datastore);
    let digest = "a".repeat(64);

    let missing = observe_target_cache_entry(datastore, "fixture", 8, &digest)
        .await
        .unwrap();
    assert_eq!(missing.status, VerifiedTargetObservationStatus::Missing);
    assert_eq!(missing.retained_bytes, 0);
    assert_eq!(missing.expected_bytes, 8);
    assert_eq!(missing.target_digest, format!("sha256:{digest}"));

    let partial_path = cache.join(format!(".target-{digest}.part"));
    std::fs::write(&partial_path, b"abc").unwrap();
    let partial = observe_target_cache_entry(datastore, "fixture", 8, &digest)
        .await
        .unwrap();
    assert_eq!(partial.status, VerifiedTargetObservationStatus::Partial);
    assert_eq!(partial.retained_bytes, 3);

    std::fs::remove_file(partial_path).unwrap();
    std::fs::write(cache.join(&digest), b"12345678").unwrap();
    let complete = observe_target_cache_entry(datastore, "fixture", 8, &digest)
        .await
        .unwrap();
    assert_eq!(complete.status, VerifiedTargetObservationStatus::Complete);
    assert_eq!(complete.retained_bytes, 8);
}

#[tokio::test]
async fn exact_target_observation_fails_closed_on_ambiguous_or_oversized_state() {
    let temporary = tempfile::tempdir().unwrap();
    let datastore = temporary.path();
    let cache = cache_directory(datastore);
    let digest = "b".repeat(64);
    std::fs::write(cache.join(&digest), b"1234").unwrap();
    std::fs::write(cache.join(format!(".target-{digest}.part")), b"12").unwrap();

    let ambiguous = observe_target_cache_entry(datastore, "fixture", 4, &digest)
        .await
        .unwrap_err();
    assert_eq!(
        ambiguous.code,
        "use.extension.registry_target_cache_invalid"
    );

    std::fs::remove_file(cache.join(&digest)).unwrap();
    std::fs::write(cache.join(format!(".target-{digest}.part")), b"12345").unwrap();
    let oversized = observe_target_cache_entry(datastore, "fixture", 4, &digest)
        .await
        .unwrap_err();
    assert_eq!(
        oversized.code,
        "use.extension.registry_target_cache_invalid"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn exact_target_observation_rejects_links_without_following_them() {
    use std::os::unix::fs::symlink;

    let temporary = tempfile::tempdir().unwrap();
    let datastore = temporary.path();
    let cache = cache_directory(datastore);
    let digest = "c".repeat(64);
    let outside = temporary.path().join("outside");
    std::fs::write(&outside, b"secret").unwrap();
    symlink(&outside, cache.join(format!(".target-{digest}.part"))).unwrap();

    let error = observe_target_cache_entry(datastore, "fixture", 8, &digest)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.registry_target_cache_invalid");
    assert_eq!(std::fs::read(outside).unwrap(), b"secret");
}
