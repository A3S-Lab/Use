use std::path::Path;

use super::*;

#[tokio::test]
async fn retention_removes_oldest_only_after_exact_plan_confirmation() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let backup_directory = temporary.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let manager = StateBackupManager::new(paths.clone());

    for (name, bytes) in [
        ("001.a3s-use-state-backup", b"one".as_slice()),
        ("002.a3s-use-state-backup", b"two".as_slice()),
        ("003.a3s-use-state-backup", b"three".as_slice()),
    ] {
        write_fixture_state(&paths, bytes);
        manager.backup(backup_directory.join(name)).await.unwrap();
    }

    let policy = StateBackupRetentionPolicy::new(2, 1024 * 1024 * 1024).unwrap();
    let stale = manager
        .plan_backup_retention(&backup_directory, policy)
        .await
        .unwrap();
    assert_eq!(stale.schema, A3S_USE_STATE_BACKUP_RETENTION_PLAN_SCHEMA);
    assert_eq!(stale.remove.len(), 1);
    assert_eq!(stale.remove[0].file_name, "001.a3s-use-state-backup");
    assert_eq!(stale.retain.len(), 2);
    let stale_digest = stale.descriptor_digest().unwrap();
    assert!(!serde_json::to_string(&stale)
        .unwrap()
        .contains(temporary.path().to_str().unwrap()));

    write_fixture_state(&paths, b"four");
    manager
        .backup(backup_directory.join("004.a3s-use-state-backup"))
        .await
        .unwrap();
    let error = manager
        .apply_backup_retention(&backup_directory, policy, &stale_digest)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_retention_plan_mismatch");
    assert!(backup_directory.join("001.a3s-use-state-backup").is_file());

    let plan = manager
        .plan_backup_retention(&backup_directory, policy)
        .await
        .unwrap();
    assert_eq!(
        plan.remove
            .iter()
            .map(|entry| entry.file_name.as_str())
            .collect::<Vec<_>>(),
        vec!["001.a3s-use-state-backup", "002.a3s-use-state-backup"]
    );
    let result = manager
        .apply_backup_retention(&backup_directory, policy, plan.descriptor_digest().unwrap())
        .await
        .unwrap();
    assert_eq!(result.schema, A3S_USE_STATE_BACKUP_RETENTION_RESULT_SCHEMA);
    assert!(result.changed);
    assert_eq!(result.removed, plan.remove);
    assert_eq!(result.retained_backup_count, 2);
    assert!(!backup_directory.join("001.a3s-use-state-backup").exists());
    assert!(!backup_directory.join("002.a3s-use-state-backup").exists());
    assert!(backup_directory.join("003.a3s-use-state-backup").is_file());
    assert!(backup_directory.join("004.a3s-use-state-backup").is_file());
}

#[tokio::test]
async fn retention_policy_is_bounded_and_preserves_two_verified_backups() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StateBackupRetentionPolicy>();
    assert_send_sync::<StateBackupRetentionPlan>();
    assert_send_sync::<StateBackupRetentionResult>();

    assert_eq!(
        StateBackupRetentionPolicy::new(1, 1).unwrap_err().code,
        "use.state_backup_retention_policy_invalid"
    );
    assert_eq!(
        StateBackupRetentionPolicy::new(2, 0).unwrap_err().code,
        "use.state_backup_retention_policy_invalid"
    );

    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let backup_directory = temporary.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let manager = StateBackupManager::new(paths.clone());
    for (name, bytes) in [
        ("001.a3s-use-state-backup", b"one".as_slice()),
        ("002.a3s-use-state-backup", b"two".as_slice()),
    ] {
        write_fixture_state(&paths, bytes);
        manager.backup(backup_directory.join(name)).await.unwrap();
    }
    let too_small = StateBackupRetentionPolicy::new(2, 1).unwrap();
    let error = manager
        .plan_backup_retention(&backup_directory, too_small)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_retention_policy_unsatisfied");
    assert!(backup_directory.join("001.a3s-use-state-backup").is_file());
    assert!(backup_directory.join("002.a3s-use-state-backup").is_file());
}

#[tokio::test]
async fn retention_rejects_tampered_linked_and_owned_state_candidates() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let backup_directory = temporary.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    let manager = StateBackupManager::new(paths.clone());
    write_fixture_state(&paths, b"verified");
    let backup = backup_directory.join("verified.a3s-use-state-backup");
    manager.backup(&backup).await.unwrap();
    let mut bytes = std::fs::read(&backup).unwrap();
    *bytes.last_mut().unwrap() ^= 0xff;
    std::fs::write(&backup, bytes).unwrap();
    let error = manager
        .plan_backup_retention(&backup_directory, StateBackupRetentionPolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_invalid");

    std::fs::remove_file(&backup).unwrap();
    #[cfg(any(unix, windows))]
    {
        let outside = temporary.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("sentinel"), b"outside").unwrap();
        crate::test_filesystem::create_directory_link(
            &outside,
            &backup_directory.join("linked.a3s-use-state-backup"),
        );
        let error = manager
            .plan_backup_retention(&backup_directory, StateBackupRetentionPolicy::default())
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.state_backup_retention_directory_invalid");
        assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"outside");
    }

    let owned = paths.state_root().join("retained-backups");
    std::fs::create_dir_all(&owned).unwrap();
    let error = manager
        .plan_backup_retention(owned, StateBackupRetentionPolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_retention_directory_invalid");
}

#[tokio::test]
async fn coordinated_backup_and_retention_share_one_external_directory_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let backup_directory = temporary.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();
    write_fixture_state(&paths, b"locked");
    let _lock = super::retention::BackupDirectoryLock::acquire(&backup_directory).unwrap();
    let manager = StateBackupManager::new(paths);

    let error = manager
        .backup(backup_directory.join("locked.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_retention_busy");
    let error = manager
        .plan_backup_retention(&backup_directory, StateBackupRetentionPolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_retention_busy");
}

fn fixture_paths(root: &Path) -> a3s_use_extension::ExtensionPaths {
    a3s_use_extension::ExtensionPaths::new(root.join("data"), root.join("state"))
}

fn write_fixture_state(paths: &a3s_use_extension::ExtensionPaths, bytes: &[u8]) {
    let path = paths
        .state_root()
        .join("registry-trust-roots/sha256/root.json");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}
