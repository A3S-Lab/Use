use std::fs::{File, FileTimes};
use std::io::Write;
use std::time::Duration;

use super::*;

#[test]
fn cache_entry_names_are_exact_and_portable() {
    let digest = "a".repeat(64);
    assert!(valid_digest_name(&"a".repeat(64)));
    assert!(!valid_digest_name(&"A".repeat(64)));
    assert!(!valid_digest_name(&"a".repeat(63)));
    assert_eq!(
        valid_observation_name(&format!("{digest}.json")),
        Some(digest.as_str())
    );
    assert!(valid_temporary_name(".target-123-456.tmp"));
    assert!(!valid_temporary_name(".target-../456.tmp"));
    assert_eq!(
        valid_partial_name(&format!(".target-{digest}.part")),
        Some(digest.as_str())
    );
    assert!(valid_partial_name(".target-short.part").is_none());
    assert!(!valid_temporary_name("unowned.tmp"));
}

#[tokio::test]
async fn staging_capacity_overflow_fails_before_disk_inspection() {
    let policy = VerifiedTargetCachePolicy::new(1, 1, 1).unwrap();
    let error = ensure_staging_capacity(Path::new("."), u64::MAX, policy)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.extension.registry_target_cache_policy_exceeded"
    );
}

#[tokio::test]
async fn prune_removes_stale_files_then_the_oldest_source_observation() {
    let temporary = tempfile::tempdir().unwrap();
    let old_path = write_test_observation(
        temporary.path(),
        &"a".repeat(64),
        3,
        UNIX_EPOCH + Duration::from_secs(1),
    )
    .await;
    let new_path = write_test_observation(
        temporary.path(),
        &"b".repeat(64),
        3,
        UNIX_EPOCH + Duration::from_secs(2),
    )
    .await;
    let partial_path = temporary
        .path()
        .join(format!(".target-{}.part", "c".repeat(64)));
    let stale_path = temporary.path().join(".target-123-456.tmp");
    for (path, body, modified) in [
        (
            &partial_path,
            b"pa".as_slice(),
            UNIX_EPOCH + Duration::from_secs(4),
        ),
        (
            &stale_path,
            b"stale".as_slice(),
            UNIX_EPOCH + Duration::from_secs(3),
        ),
    ] {
        let mut file = File::create(path).unwrap();
        file.write_all(body).unwrap();
        file.sync_all().unwrap();
        file.set_times(FileTimes::new().set_modified(modified))
            .unwrap();
    }
    let policy = VerifiedTargetCachePolicy::new(3, 1, 0).unwrap();

    let removed = prune_cache(temporary.path(), policy).await.unwrap();

    assert_eq!(removed.target_entries, 1);
    assert_eq!(removed.target_bytes, 3);
    assert_eq!(removed.partial_entries, 1);
    assert_eq!(removed.partial_bytes, 2);
    assert_eq!(removed.stale_entries, 1);
    assert_eq!(removed.stale_bytes, 5);
    assert!(!old_path.exists());
    assert!(new_path.is_file());
    assert!(!partial_path.exists());
    assert!(!stale_path.exists());
    let usage = inspect_cache(temporary.path()).await.unwrap();
    assert_eq!(usage.target_entries, 1);
    assert_eq!(usage.target_bytes, 3);
    assert_eq!(usage.partial_entries, 0);
    assert_eq!(usage.partial_bytes, 0);
}

#[tokio::test]
async fn admission_preserves_the_current_partial_and_reclaims_source_observations() {
    let temporary = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let partial_path = temporary.path().join(format!(".target-{digest}.part"));
    let other_target = write_test_observation(
        temporary.path(),
        &"b".repeat(64),
        3,
        UNIX_EPOCH + Duration::from_secs(1),
    )
    .await;
    std::fs::write(&partial_path, b"pa").unwrap();
    let policy = VerifiedTargetCachePolicy::new(4, 1, 0).unwrap();

    let removed = admit_target_write(temporary.path(), &digest, 4, policy, true)
        .await
        .unwrap();

    assert_eq!(removed.target_entries, 1);
    assert_eq!(removed.partial_entries, 0);
    assert!(partial_path.is_file());
    assert!(!other_target.exists());
}

#[tokio::test]
async fn admission_removes_a_partial_redundant_with_a_source_observation() {
    let temporary = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let target_path = write_test_observation(
        temporary.path(),
        &digest,
        6,
        UNIX_EPOCH + Duration::from_secs(1),
    )
    .await;
    let partial_path = temporary.path().join(format!(".target-{digest}.part"));
    std::fs::write(&partial_path, b"part").unwrap();
    let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

    let removed = admit_target_write(temporary.path(), &digest, 6, policy, false)
        .await
        .unwrap();

    assert_eq!(removed.partial_entries, 1);
    assert_eq!(removed.partial_bytes, 4);
    assert!(target_path.is_file());
    assert!(!partial_path.exists());
}

#[tokio::test]
async fn cached_blob_admission_replaces_an_unobserved_partial_in_the_policy_ledger() {
    let temporary = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let partial_path = temporary.path().join(format!(".target-{digest}.part"));
    std::fs::write(&partial_path, b"part").unwrap();
    let policy = VerifiedTargetCachePolicy::new(6, 1, 0).unwrap();

    let removed = admit_target_write(temporary.path(), &digest, 6, policy, false)
        .await
        .unwrap();

    assert_eq!(removed.partial_entries, 1);
    assert_eq!(removed.partial_bytes, 4);
    assert!(!partial_path.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn prune_waits_for_a_transient_scanner_lock_on_a_stale_entry() {
    let temporary = tempfile::tempdir().unwrap();
    let stale_path = temporary.path().join(".target-123-456.tmp");
    std::fs::write(&stale_path, b"stale").unwrap();
    let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&stale_path);
    let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();
    let mut prune = Box::pin(prune_cache(temporary.path(), policy));

    tokio::select! {
        result = &mut prune => {
            panic!("stale cleanup completed while the scanner denied delete sharing: {result:?}")
        }
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }

    drop(scanner);
    let removed = prune.await.unwrap();
    assert_eq!(removed.stale_entries, 1);
    assert_eq!(removed.stale_bytes, 5);
    assert!(!stale_path.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn prune_waits_for_a_transient_scanner_lock_on_a_partial() {
    let temporary = tempfile::tempdir().unwrap();
    let partial_path = temporary
        .path()
        .join(format!(".target-{}.part", "a".repeat(64)));
    std::fs::write(&partial_path, b"partial").unwrap();
    let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&partial_path);
    let policy = VerifiedTargetCachePolicy::new(1, 1, 0).unwrap();
    let mut prune = Box::pin(prune_cache(temporary.path(), policy));

    tokio::select! {
        result = &mut prune => {
            panic!("partial cleanup completed while the scanner denied delete sharing: {result:?}")
        }
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }

    drop(scanner);
    let removed = prune.await.unwrap();
    assert_eq!(removed.partial_entries, 1);
    assert_eq!(removed.partial_bytes, 7);
    assert!(!partial_path.exists());
}

#[cfg(windows)]
#[tokio::test]
async fn persistent_scanner_lock_bounds_prune_and_preserves_the_source_observation() {
    let temporary = tempfile::tempdir().unwrap();
    let old_path = write_test_observation(
        temporary.path(),
        &"a".repeat(64),
        3,
        UNIX_EPOCH + Duration::from_secs(1),
    )
    .await;
    let new_path = write_test_observation(
        temporary.path(),
        &"b".repeat(64),
        3,
        UNIX_EPOCH + Duration::from_secs(2),
    )
    .await;
    let stale_path = temporary.path().join(".target-123-456.tmp");
    std::fs::write(&stale_path, b"stale").unwrap();
    let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&old_path);
    let policy = VerifiedTargetCachePolicy::new(3, 1, 0).unwrap();

    let started = std::time::Instant::now();
    let error = prune_cache(temporary.path(), policy)
        .await
        .expect_err("a persistent scanner lock must stop at the retry bound");
    let elapsed = started.elapsed();

    assert_eq!(error.code, "use.extension.io");
    assert!(elapsed >= Duration::from_secs(2));
    assert!(elapsed < Duration::from_secs(10));
    assert!(!stale_path.exists());
    assert!(old_path.is_file());
    assert!(new_path.is_file());

    drop(scanner);
    let removed = prune_cache(temporary.path(), policy).await.unwrap();
    assert_eq!(removed.target_entries, 1);
    assert_eq!(removed.target_bytes, 3);
    assert_eq!(removed.stale_entries, 0);
    assert!(!old_path.exists());
    assert!(new_path.is_file());
}

#[tokio::test]
async fn inventory_rejects_unowned_entries() {
    let temporary = tempfile::tempdir().unwrap();
    let unowned = temporary.path().join("unowned");
    std::fs::write(&unowned, b"x").unwrap();
    assert_eq!(
        inspect_cache(temporary.path()).await.unwrap_err().code,
        "use.extension.registry_target_cache_invalid"
    );
    std::fs::remove_file(unowned).unwrap();
    std::fs::write(temporary.path().join("a".repeat(64)), b"legacy raw bytes").unwrap();
    assert_eq!(
        inspect_cache(temporary.path()).await.unwrap_err().code,
        "use.extension.registry_target_cache_invalid"
    );
}

async fn write_test_observation(
    cache: &Path,
    digest: &str,
    expected_bytes: u64,
    modified: SystemTime,
) -> PathBuf {
    record::write_observation(cache, digest, expected_bytes)
        .await
        .unwrap();
    let path = record::observation_path(cache, digest);
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    path
}
