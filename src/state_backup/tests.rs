use std::path::Path;

use sha2::{Digest, Sha256};

use super::*;

#[tokio::test]
async fn coordinated_backup_is_path_free_deterministic_and_offline_verifiable() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.data_root().join("extensions/acme/tool/package/bin")).unwrap();
    std::fs::write(
        paths
            .data_root()
            .join("extensions/acme/tool/package/bin/tool"),
        b"portable tool bytes",
    )
    .unwrap();
    std::fs::create_dir_all(paths.state_root().join("remote-registries/fixture/cache")).unwrap();
    std::fs::write(
        paths
            .state_root()
            .join("remote-registries/fixture/cache/timestamp.json"),
        b"trusted cache bytes",
    )
    .unwrap();
    std::fs::write(paths.state_root().join(".installation-mutation.lock"), b"").unwrap();

    let first = temporary.path().join("first.a3s-use-state-backup");
    let manager = StateBackupManager::new(paths.clone());
    let manifest = manager.backup(&first).await.unwrap();
    assert_eq!(manifest.schema, A3S_USE_STATE_BACKUP_SCHEMA);
    assert_eq!(manifest.file_count, 2);
    assert_eq!(manifest.entries.len(), 2);
    assert_eq!(manifest.families.len(), 2);
    assert!(manifest.inventory_digest.starts_with("sha256:"));
    assert_eq!(manifest.authority.registry_generation, 0);
    assert!(manifest.authority.packages.is_empty());
    assert!(manifest
        .entries
        .iter()
        .all(|entry| !entry.path.starts_with('/') && !entry.path.contains("..")));
    let encoded = serde_json::to_string(&manifest).unwrap();
    assert!(!encoded.contains(temporary.path().to_str().unwrap()));
    assert!(!encoded.contains(".installation-mutation.lock"));
    assert!(!encoded.contains(".maintenance.lock"));

    let verified = StateBackupManager::verify_backup(&first).await.unwrap();
    assert_eq!(verified, manifest);

    let second = temporary.path().join("second.a3s-use-state-backup");
    let second_manifest = manager.backup(&second).await.unwrap();
    assert_eq!(second_manifest.entries, manifest.entries);
    assert_eq!(second_manifest.families, manifest.families);
    assert_eq!(second_manifest.inventory_digest, manifest.inventory_digest);
    assert_eq!(
        std::fs::read(first).unwrap(),
        std::fs::read(second).unwrap()
    );
}

#[tokio::test]
async fn coordinated_backup_accepts_only_terminal_durable_operation_records() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let lifecycle = paths
        .state_root()
        .join("operations/plugins/user/scope/acme/tool/active.json");
    std::fs::create_dir_all(lifecycle.parent().unwrap()).unwrap();
    std::fs::write(&lifecycle, br#"{"status":"completed"}"#).unwrap();
    let completed = temporary.path().join("completed.a3s-use-state-backup");
    let manifest = StateBackupManager::new(paths.clone())
        .backup(&completed)
        .await
        .unwrap();
    assert!(manifest
        .entries
        .iter()
        .any(|entry| entry.family == StateBackupFamily::LifecycleOperations));

    std::fs::write(&lifecycle, br#"{"status":"applying"}"#).unwrap();
    let error = StateBackupManager::new(paths)
        .backup(temporary.path().join("applying.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_nonterminal");
}

#[tokio::test]
async fn coordinated_backup_rejects_nonterminal_and_unknown_state() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.state_root().join("remote-registries/fixture/cache")).unwrap();
    std::fs::write(
        paths
            .state_root()
            .join("remote-registries/fixture/cache/.target-1-1.tmp"),
        b"partial",
    )
    .unwrap();
    let error = StateBackupManager::new(paths.clone())
        .backup(temporary.path().join("partial.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_nonterminal");

    std::fs::remove_dir_all(paths.state_root().join("remote-registries")).unwrap();
    std::fs::create_dir_all(paths.state_root().join("unknown-family")).unwrap();
    std::fs::write(
        paths.state_root().join("unknown-family/evidence.json"),
        b"{}",
    )
    .unwrap();
    let error = StateBackupManager::new(paths)
        .backup(temporary.path().join("unknown.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_layout_unsupported");
}

#[tokio::test]
async fn coordinated_backup_rejects_an_active_restore_and_links() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.state_root()).unwrap();
    std::fs::write(
        paths
            .state_root()
            .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER),
        b"{}",
    )
    .unwrap();
    let error = StateBackupManager::new(paths.clone())
        .backup(temporary.path().join("active.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_nonterminal");
    std::fs::remove_file(
        paths
            .state_root()
            .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER),
    )
    .unwrap();

    #[cfg(any(unix, windows))]
    {
        let outside = temporary.path().join("outside");
        std::fs::create_dir(&outside).unwrap();
        std::fs::write(outside.join("secret.json"), b"secret").unwrap();
        std::fs::create_dir_all(paths.state_root().join("grants")).unwrap();
        crate::test_filesystem::create_directory_link(
            &outside,
            &paths.state_root().join("grants/linked"),
        );
        let error = StateBackupManager::new(paths)
            .backup(temporary.path().join("linked.a3s-use-state-backup"))
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.state_backup_path_invalid");
        assert_eq!(
            std::fs::read(outside.join("secret.json")).unwrap(),
            b"secret"
        );
    }
}

#[tokio::test]
async fn backup_verification_rejects_tampering_and_creation_never_overwrites() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.state_root().join("registry-trust-roots/sha256")).unwrap();
    std::fs::write(
        paths
            .state_root()
            .join("registry-trust-roots/sha256/root.json"),
        b"root",
    )
    .unwrap();
    let destination = temporary.path().join("state.a3s-use-state-backup");
    let manager = StateBackupManager::new(paths);
    manager.backup(&destination).await.unwrap();
    let original = std::fs::read(&destination).unwrap();
    let error = manager.backup(&destination).await.unwrap_err();
    assert_eq!(error.code, "use.state_backup_exists");
    assert_eq!(std::fs::read(&destination).unwrap(), original);

    let mut tampered = original;
    *tampered.last_mut().unwrap() ^= 0xff;
    let tampered_path = temporary.path().join("tampered.a3s-use-state-backup");
    std::fs::write(&tampered_path, tampered).unwrap();
    let error = StateBackupManager::verify_backup(&tampered_path)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_invalid");
}

#[tokio::test]
async fn coordinated_backup_rejects_a_destination_inside_owned_state() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.state_root()).unwrap();
    let error = StateBackupManager::new(paths.clone())
        .backup(paths.state_root().join("recursive.a3s-use-state-backup"))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_path_invalid");
}

#[tokio::test]
async fn backup_verification_rejects_noncanonical_manifest_encoding() {
    const MAGIC: &[u8] = b"A3S-USE-STATE-BACKUP-V1\n";

    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.state_root().join("registry-trust-roots/sha256")).unwrap();
    std::fs::write(
        paths
            .state_root()
            .join("registry-trust-roots/sha256/root.json"),
        b"root",
    )
    .unwrap();
    let original_path = temporary.path().join("canonical.a3s-use-state-backup");
    StateBackupManager::new(paths)
        .backup(&original_path)
        .await
        .unwrap();
    let original = std::fs::read(original_path).unwrap();
    let length_start = MAGIC.len();
    let length_end = length_start + 8;
    let manifest_length =
        u64::from_be_bytes(original[length_start..length_end].try_into().unwrap()) as usize;
    let manifest_start = length_end + 32;
    let manifest_end = manifest_start + manifest_length;
    let value: serde_json::Value =
        serde_json::from_slice(&original[manifest_start..manifest_end]).unwrap();
    let noncanonical = serde_json::to_vec_pretty(&value).unwrap();
    let mut rebuilt = Vec::new();
    rebuilt.extend_from_slice(MAGIC);
    rebuilt.extend_from_slice(&(noncanonical.len() as u64).to_be_bytes());
    rebuilt.extend_from_slice(Sha256::digest(&noncanonical).as_slice());
    rebuilt.extend_from_slice(&noncanonical);
    rebuilt.extend_from_slice(&original[manifest_end..]);
    let path = temporary.path().join("noncanonical.a3s-use-state-backup");
    std::fs::write(&path, rebuilt).unwrap();
    let error = StateBackupManager::verify_backup(path).await.unwrap_err();
    assert_eq!(error.code, "use.state_backup_invalid");
}

#[tokio::test]
async fn coordinated_backup_waits_for_shared_state_users() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let shared = StateMaintenanceLock::new(paths.state_root())
        .acquire_shared()
        .await
        .unwrap();
    let manager = StateBackupManager::new(paths);
    let destination = temporary.path().join("fenced.a3s-use-state-backup");
    let mut backup = tokio::spawn(async move { manager.backup(destination).await });
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), &mut backup)
            .await
            .is_err()
    );
    drop(shared);
    let manifest = tokio::time::timeout(std::time::Duration::from_secs(2), backup)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(manifest.file_count, 0);
}

fn fixture_paths(root: &Path) -> a3s_use_extension::ExtensionPaths {
    a3s_use_extension::ExtensionPaths::new(root.join("data"), root.join("state"))
}
