use std::fs::{File, FileTimes};
use std::io::Write;
use std::time::Duration;

use super::*;

#[test]
fn cache_entry_names_are_exact_and_portable() {
    assert!(valid_digest_name(&"a".repeat(64)));
    assert!(!valid_digest_name(&"A".repeat(64)));
    assert!(!valid_digest_name(&"a".repeat(63)));
    assert!(valid_temporary_name(".target-123-456.tmp"));
    assert!(!valid_temporary_name(".target-../456.tmp"));
    let digest = "a".repeat(64);
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
async fn prune_removes_stale_files_then_the_oldest_verified_target() {
    let temporary = tempfile::tempdir().unwrap();
    let old_path = temporary.path().join("a".repeat(64));
    let new_path = temporary.path().join("b".repeat(64));
    let partial_path = temporary
        .path()
        .join(format!(".target-{}.part", "c".repeat(64)));
    let stale_path = temporary.path().join(".target-123-456.tmp");
    for (path, body, modified) in [
        (
            &old_path,
            b"old".as_slice(),
            UNIX_EPOCH + Duration::from_secs(1),
        ),
        (
            &new_path,
            b"new".as_slice(),
            UNIX_EPOCH + Duration::from_secs(2),
        ),
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
async fn admission_preserves_the_current_partial_and_reclaims_verified_targets() {
    let temporary = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let partial_path = temporary.path().join(format!(".target-{digest}.part"));
    let other_target = temporary.path().join("b".repeat(64));
    std::fs::write(&partial_path, b"pa").unwrap();
    std::fs::write(&other_target, b"old").unwrap();
    let policy = VerifiedTargetCachePolicy::new(4, 1, 0).unwrap();

    let removed = admit_target_write(temporary.path(), &digest, 4, policy)
        .await
        .unwrap();

    assert_eq!(removed.target_entries, 1);
    assert_eq!(removed.partial_entries, 0);
    assert!(partial_path.is_file());
    assert!(!other_target.exists());
}

#[tokio::test]
async fn admission_removes_a_partial_redundant_with_a_verified_target() {
    let temporary = tempfile::tempdir().unwrap();
    let digest = "a".repeat(64);
    let target_path = temporary.path().join(&digest);
    let partial_path = temporary.path().join(format!(".target-{digest}.part"));
    std::fs::write(&target_path, b"target").unwrap();
    std::fs::write(&partial_path, b"part").unwrap();
    let policy = VerifiedTargetCachePolicy::new(1024, 4, 0).unwrap();

    let removed = admit_target_write(temporary.path(), &digest, 6, policy)
        .await
        .unwrap();

    assert_eq!(removed.partial_entries, 1);
    assert_eq!(removed.partial_bytes, 4);
    assert!(target_path.is_file());
    assert!(!partial_path.exists());
}

#[tokio::test]
async fn inventory_rejects_unowned_entries() {
    let temporary = tempfile::tempdir().unwrap();
    std::fs::write(temporary.path().join("unowned"), b"x").unwrap();
    assert_eq!(
        inspect_cache(temporary.path()).await.unwrap_err().code,
        "use.extension.registry_target_cache_invalid"
    );
}
