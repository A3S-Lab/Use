use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::*;

mod security;

#[tokio::test]
async fn plan_is_path_free_and_classifies_add_replace_remove_and_retain() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let knowledge = paths.state_root().join("knowledge");
    std::fs::create_dir_all(&knowledge).unwrap();
    std::fs::write(knowledge.join("add.bin"), b"candidate add").unwrap();
    std::fs::write(knowledge.join("replace.bin"), b"candidate replacement").unwrap();
    std::fs::write(knowledge.join("retain.bin"), b"retained").unwrap();

    let backup_path = temporary.path().join("candidate.a3s-use-state-backup");
    let backup = StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    std::fs::remove_file(knowledge.join("add.bin")).unwrap();
    std::fs::write(knowledge.join("replace.bin"), b"live replacement").unwrap();
    std::fs::write(knowledge.join("remove.bin"), b"live only").unwrap();

    let plan = StateRestoreManager::new(paths)
        .plan_restore(&backup_path)
        .await
        .unwrap();

    assert_eq!(plan.status, StateRestorePlanStatus::Required);
    assert_eq!(plan.backup, backup);
    assert_eq!(
        plan.actions
            .iter()
            .map(|action| (action.path.as_str(), action.action))
            .collect::<Vec<_>>(),
        vec![
            ("knowledge/add.bin", StateRestoreActionKind::Add),
            ("knowledge/remove.bin", StateRestoreActionKind::Remove),
            ("knowledge/replace.bin", StateRestoreActionKind::Replace),
            ("knowledge/retain.bin", StateRestoreActionKind::Retain),
        ]
    );
    assert_eq!(plan.summary.add_files, 1);
    assert_eq!(plan.summary.add_bytes, b"candidate add".len() as u64);
    assert_eq!(plan.summary.replace_files, 1);
    assert_eq!(
        plan.summary.replace_bytes,
        b"candidate replacement".len() as u64
    );
    assert_eq!(plan.summary.remove_files, 1);
    assert_eq!(plan.summary.remove_bytes, b"live only".len() as u64);
    assert_eq!(plan.summary.retain_files, 1);
    assert_eq!(plan.summary.retain_bytes, b"retained".len() as u64);
    assert!(plan
        .actions
        .iter()
        .all(|action| !action.path.starts_with('/')));
    let encoded = String::from_utf8(plan.canonical_bytes().unwrap()).unwrap();
    assert!(!encoded.contains(temporary.path().to_str().unwrap()));
    assert!(!encoded.contains(backup_path.to_str().unwrap()));
    assert_eq!(
        plan.descriptor_digest().unwrap(),
        sha256(encoded.as_bytes())
    );
}

#[tokio::test]
async fn unchanged_installation_produces_a_no_change_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let knowledge = paths.state_root().join("knowledge");
    std::fs::create_dir_all(&knowledge).unwrap();
    std::fs::write(knowledge.join("stable.bin"), b"stable").unwrap();
    let backup_path = temporary.path().join("stable.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();

    let plan = StateRestoreManager::new(paths)
        .plan_restore(backup_path)
        .await
        .unwrap();

    assert_eq!(plan.status, StateRestorePlanStatus::NoChange);
    assert_eq!(plan.summary.add_files, 0);
    assert_eq!(plan.summary.replace_files, 0);
    assert_eq!(plan.summary.remove_files, 0);
    assert_eq!(plan.summary.retain_files, 1);
    assert_eq!(plan.actions[0].action, StateRestoreActionKind::Retain);
    plan.validate().unwrap();
}

#[tokio::test]
async fn apply_creates_exact_rollback_publishes_inventory_and_replays_terminally() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let knowledge = paths.state_root().join("knowledge");
    std::fs::create_dir_all(&knowledge).unwrap();
    std::fs::write(knowledge.join("add.bin"), b"candidate add").unwrap();
    std::fs::write(knowledge.join("replace.bin"), b"candidate replacement").unwrap();
    std::fs::write(knowledge.join("retain.bin"), b"retained").unwrap();
    let backup_path = temporary.path().join("candidate.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();

    std::fs::remove_file(knowledge.join("add.bin")).unwrap();
    std::fs::write(knowledge.join("replace.bin"), b"live replacement").unwrap();
    std::fs::write(knowledge.join("remove.bin"), b"live only").unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let rollback_path = temporary.path().join("rollback.a3s-use-state-backup");

    let result = manager
        .apply_restore(&backup_path, &rollback_path, &plan_digest)
        .await
        .unwrap();

    assert!(result.changed);
    assert_eq!(result.plan_digest, plan_digest);
    assert_eq!(
        result.rollback_backup_manifest_digest,
        Some(
            StateBackupManager::verify_backup(&rollback_path)
                .await
                .unwrap()
                .descriptor_digest()
                .unwrap()
        )
    );
    assert_eq!(
        std::fs::read(knowledge.join("add.bin")).unwrap(),
        b"candidate add"
    );
    assert_eq!(
        std::fs::read(knowledge.join("replace.bin")).unwrap(),
        b"candidate replacement"
    );
    assert_eq!(
        std::fs::read(knowledge.join("retain.bin")).unwrap(),
        b"retained"
    );
    assert!(!knowledge.join("remove.bin").exists());
    assert!(!paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
    let digest = plan_digest.strip_prefix("sha256:").unwrap();
    assert!(!paths
        .state_root()
        .join(format!(".state-restore-{digest}"))
        .exists());
    assert!(!paths
        .data_root()
        .join(format!(".state-restore-{digest}"))
        .exists());
    let operation: StateRestoreOperation = serde_json::from_slice(
        &std::fs::read(
            paths
                .state_root()
                .join("operations/state-restores")
                .join(digest)
                .join("operation.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(operation.status, StateRestoreOperationStatus::Completed);

    let replay = manager
        .apply_restore(&backup_path, &rollback_path, &plan_digest)
        .await
        .unwrap();
    assert_eq!(replay, result);
    assert!(!paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
}

#[tokio::test]
async fn no_change_apply_does_not_create_a_rollback_or_operation() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    std::fs::create_dir_all(paths.state_root().join("knowledge")).unwrap();
    std::fs::write(paths.state_root().join("knowledge/stable.bin"), b"stable").unwrap();
    let backup_path = temporary.path().join("stable.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let rollback_path = temporary
        .path()
        .join("unused-rollback.a3s-use-state-backup");

    let result = manager
        .apply_restore(backup_path, &rollback_path, &plan_digest)
        .await
        .unwrap();

    assert!(!result.changed);
    assert!(result.rollback_backup_manifest_digest.is_none());
    assert!(!rollback_path.exists());
    assert!(!paths
        .state_root()
        .join("operations/state-restores")
        .exists());
}

#[tokio::test]
async fn apply_rejects_stale_live_state_before_rollback_or_marker() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let file = paths.state_root().join("knowledge/value.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"candidate").unwrap();
    let backup_path = temporary.path().join("candidate.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    std::fs::write(&file, b"reviewed live").unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    std::fs::write(&file, b"changed after review").unwrap();
    let rollback_path = temporary.path().join("rollback.a3s-use-state-backup");

    let error = manager
        .apply_restore(backup_path, &rollback_path, &plan_digest)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.state_restore_plan_mismatch");
    assert!(!rollback_path.exists());
    assert!(!paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
}

#[tokio::test]
async fn apply_rejects_archive_tampering_and_rollback_inside_owned_state() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let file = paths.state_root().join("knowledge/value.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"candidate").unwrap();
    let backup_path = temporary.path().join("candidate.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    std::fs::write(&file, b"live").unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let original = std::fs::read(&backup_path).unwrap();
    let mut tampered = original.clone();
    *tampered.last_mut().unwrap() ^= 0xff;
    std::fs::write(&backup_path, tampered).unwrap();
    let rollback_path = temporary.path().join("rollback.a3s-use-state-backup");

    let error = manager
        .apply_restore(&backup_path, &rollback_path, &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_invalid");
    assert!(!rollback_path.exists());
    std::fs::write(&backup_path, original).unwrap();

    let error = manager
        .apply_restore(
            &backup_path,
            paths
                .state_root()
                .join("knowledge/rollback.a3s-use-state-backup"),
            &plan_digest,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_restore_path_invalid");
    assert!(!paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
}

#[tokio::test]
async fn marker_only_handoff_blocks_shared_access_and_reconstructs_the_journal() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let file = paths.state_root().join("knowledge/value.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"candidate").unwrap();
    let backup_path = temporary.path().join("candidate.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    std::fs::write(&file, b"live").unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let rollback_path = temporary.path().join("rollback.a3s-use-state-backup");
    let rollback = StateBackupManager::new(paths.clone())
        .backup(&rollback_path)
        .await
        .unwrap();
    let operation = StateRestoreOperation::new(
        plan,
        plan_digest.clone(),
        rollback.descriptor_digest().unwrap(),
        now_ms().unwrap(),
    )
    .unwrap();
    manager.operations.activate(&operation).await.unwrap();

    let marker_path = paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER);
    let marker_before = std::fs::read(&marker_path).unwrap();
    let diagnostic = manager.diagnose_restore().await.unwrap();
    assert_eq!(
        diagnostic.active.as_ref().unwrap().status,
        StateRestoreDiagnosticStatus::MarkerOnly
    );
    assert!(diagnostic.operations.is_empty());
    assert_eq!(diagnostic.unrecorded_operation_directories, 0);
    assert_eq!(std::fs::read(&marker_path).unwrap(), marker_before);

    let error = StateMaintenanceLock::new(paths.state_root())
        .try_acquire_shared()
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state.maintenance_restore_active");
    assert!(manager
        .operations
        .load(&plan_digest)
        .await
        .unwrap()
        .is_none());

    let result = manager
        .apply_restore(&backup_path, &rollback_path, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(std::fs::read(file).unwrap(), b"candidate");
    assert_eq!(
        manager
            .operations
            .load(&plan_digest)
            .await
            .unwrap()
            .unwrap()
            .status,
        StateRestoreOperationStatus::Completed
    );
}

#[cfg(unix)]
#[tokio::test]
async fn restore_preserves_reviewed_unix_mode_and_read_only_evidence() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let file = paths.state_root().join("knowledge/mode.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"candidate").unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o444)).unwrap();
    let backup_path = temporary.path().join("mode.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::write(&file, b"live").unwrap();
    let manager = StateRestoreManager::new(paths);
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    manager
        .apply_restore(
            backup_path,
            temporary.path().join("rollback.a3s-use-state-backup"),
            &plan_digest,
        )
        .await
        .unwrap();

    let metadata = std::fs::metadata(&file).unwrap();
    assert_eq!(metadata.mode() & 0o7777, 0o444);
    assert!(metadata.permissions().readonly());
    assert_eq!(std::fs::read(file).unwrap(), b"candidate");
}

#[tokio::test]
async fn planning_rejects_registry_and_grant_authority_drift() {
    for authority_path in [
        "registry-trust-roots/sha256/root.json",
        "grants/user/fixture/grant.json",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temporary.path());
        let path = paths.state_root().join(authority_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"retained authority").unwrap();
        let backup_path = temporary.path().join("authority.a3s-use-state-backup");
        StateBackupManager::new(paths.clone())
            .backup(&backup_path)
            .await
            .unwrap();
        std::fs::write(path, b"drifted authority").unwrap();

        let error = StateRestoreManager::new(paths)
            .plan_restore(backup_path)
            .await
            .unwrap_err();

        assert_eq!(error.code, "use.state_restore_authority_mismatch");
    }
}

#[tokio::test]
async fn planning_rejects_version_os_and_architecture_mismatch() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let backup_path = temporary.path().join("platform.a3s-use-state-backup");
    let manifest = StateBackupManager::new(paths)
        .backup(&backup_path)
        .await
        .unwrap();

    for mutate in [
        |manifest: &mut StateBackupManifest| manifest.use_version = "999.0.0".to_owned(),
        |manifest: &mut StateBackupManifest| manifest.os = "unsupported-os".to_owned(),
        |manifest: &mut StateBackupManifest| {
            manifest.architecture = "unsupported-architecture".to_owned()
        },
    ] {
        let mut incompatible = manifest.clone();
        mutate(&mut incompatible);
        let error = validate_backup_platform(&incompatible).unwrap_err();
        assert_eq!(error.code, "use.state_restore_incompatible");
    }
}

#[tokio::test]
async fn planning_rejects_active_restore_links_and_unknown_state() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let backup_path = temporary.path().join("safe.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();

    let marker = paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER);
    std::fs::write(&marker, b"{}").unwrap();
    let error = StateRestoreManager::new(paths.clone())
        .plan_restore(&backup_path)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_nonterminal");
    std::fs::remove_file(marker).unwrap();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;

        let outside = temporary.path().join("outside.bin");
        std::fs::write(&outside, b"outside").unwrap();
        let linked = paths.state_root().join("knowledge/linked.bin");
        std::fs::create_dir_all(linked.parent().unwrap()).unwrap();
        symlink(outside, &linked).unwrap();
        let error = StateRestoreManager::new(paths.clone())
            .plan_restore(&backup_path)
            .await
            .unwrap_err();
        assert_eq!(error.code, "use.state_backup_path_invalid");
        std::fs::remove_file(linked).unwrap();
    }

    let unknown = paths.state_root().join("unknown-family/evidence.json");
    std::fs::create_dir_all(unknown.parent().unwrap()).unwrap();
    std::fs::write(unknown, b"{}").unwrap();
    let error = StateRestoreManager::new(paths)
        .plan_restore(backup_path)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_backup_layout_unsupported");
}

fn fixture_paths(root: &Path) -> ExtensionPaths {
    ExtensionPaths::new(root.join("data"), root.join("state"))
}

const RESTORE_CHILD_ROOT_ENV: &str = "A3S_USE_TEST_STATE_RESTORE_ROOT";
const RESTORE_CHILD_BACKUP_ENV: &str = "A3S_USE_TEST_STATE_RESTORE_BACKUP";
const RESTORE_CHILD_ROLLBACK_ENV: &str = "A3S_USE_TEST_STATE_RESTORE_ROLLBACK";
const RESTORE_CHILD_PLAN_DIGEST_ENV: &str = "A3S_USE_TEST_STATE_RESTORE_PLAN_DIGEST";
const RESTORE_CRASH_EXIT_CODE: i32 = 87;

#[tokio::test]
async fn every_restore_checkpoint_recovers_after_process_exit() {
    for checkpoint in [
        "rollback-captured",
        "active-marker",
        "journal-planned",
        "candidates-staged",
        "status-staged",
        "status-publishing",
        "action-0-candidate-published",
        "action-1-target-removed",
        "action-2-target-removed",
        "action-2-candidate-published",
        "status-published",
        "candidate-root-1-removed",
        "status-candidates-removed",
        "status-verified",
        "status-completed",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let paths = fixture_paths(temporary.path());
        let knowledge = paths.state_root().join("knowledge");
        std::fs::create_dir_all(&knowledge).unwrap();
        std::fs::write(knowledge.join("add.bin"), b"candidate add").unwrap();
        std::fs::write(knowledge.join("replace.bin"), b"candidate replacement").unwrap();
        std::fs::write(knowledge.join("retain.bin"), b"retained").unwrap();
        let backup_path = temporary
            .path()
            .join(format!("candidate-{checkpoint}.a3s-use-state-backup"));
        StateBackupManager::new(paths.clone())
            .backup(&backup_path)
            .await
            .unwrap();
        std::fs::remove_file(knowledge.join("add.bin")).unwrap();
        std::fs::write(knowledge.join("replace.bin"), b"live replacement").unwrap();
        std::fs::write(knowledge.join("remove.bin"), b"live only").unwrap();
        let manager = StateRestoreManager::new(paths.clone());
        let plan = manager.plan_restore(&backup_path).await.unwrap();
        let plan_digest = plan.descriptor_digest().unwrap();
        let rollback_path = temporary
            .path()
            .join(format!("rollback-{checkpoint}.a3s-use-state-backup"));

        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .arg("state_restore_checkpoint_crash_child")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(RESTORE_CHILD_ROOT_ENV, temporary.path())
            .env(RESTORE_CHILD_BACKUP_ENV, &backup_path)
            .env(RESTORE_CHILD_ROLLBACK_ENV, &rollback_path)
            .env(RESTORE_CHILD_PLAN_DIGEST_ENV, &plan_digest)
            .env(RESTORE_CRASH_CHECKPOINT_ENV, checkpoint)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(RESTORE_CRASH_EXIT_CODE),
            "restore child did not exit at {checkpoint}: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let evidence_before = snapshot_regular_files(paths.state_root());
        let diagnostic = manager.diagnose_restore().await.unwrap();
        assert_eq!(snapshot_regular_files(paths.state_root()), evidence_before);
        if checkpoint == "rollback-captured" {
            assert!(diagnostic.active.is_none());
        } else {
            assert_eq!(diagnostic.active.as_ref().unwrap().plan_digest, plan_digest);
        }
        if checkpoint == "status-staged" {
            let digest = plan_digest.strip_prefix("sha256:").unwrap();
            let candidate = paths.state_root().join(format!(".state-restore-{digest}"));
            assert_eq!(
                snapshot_regular_files(&candidate)
                    .into_keys()
                    .collect::<Vec<_>>(),
                vec![
                    PathBuf::from("knowledge/add.bin"),
                    PathBuf::from("knowledge/replace.bin"),
                ]
            );
            assert!(!paths
                .data_root()
                .join(format!(".state-restore-{digest}"))
                .exists());
        }
        if checkpoint == "candidates-staged" {
            std::fs::remove_file(&backup_path).unwrap();
        }

        let result = manager
            .apply_restore(&backup_path, &rollback_path, &plan_digest)
            .await
            .unwrap_or_else(|error| panic!("failed to recover {checkpoint}: {error}"));
        assert!(result.changed);
        assert_eq!(
            std::fs::read(knowledge.join("add.bin")).unwrap(),
            b"candidate add"
        );
        assert_eq!(
            std::fs::read(knowledge.join("replace.bin")).unwrap(),
            b"candidate replacement"
        );
        assert_eq!(
            std::fs::read(knowledge.join("retain.bin")).unwrap(),
            b"retained"
        );
        assert!(!knowledge.join("remove.bin").exists());
        assert!(!paths
            .state_root()
            .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
            .exists());
        let rollback = StateBackupManager::verify_backup(&rollback_path)
            .await
            .unwrap();
        assert_eq!(rollback.inventory_digest, plan.before_inventory_digest);
        let replay = manager
            .apply_restore(&backup_path, &rollback_path, &plan_digest)
            .await
            .unwrap();
        assert_eq!(replay, result);
    }
}

#[tokio::test]
#[ignore]
async fn state_restore_checkpoint_crash_child() {
    let Some(root) = std::env::var_os(RESTORE_CHILD_ROOT_ENV).map(PathBuf::from) else {
        return;
    };
    let backup = PathBuf::from(std::env::var_os(RESTORE_CHILD_BACKUP_ENV).unwrap());
    let rollback = PathBuf::from(std::env::var_os(RESTORE_CHILD_ROLLBACK_ENV).unwrap());
    let plan_digest = std::env::var(RESTORE_CHILD_PLAN_DIGEST_ENV).unwrap();
    StateRestoreManager::new(fixture_paths(&root))
        .apply_restore(backup, rollback, &plan_digest)
        .await
        .unwrap();
}

fn snapshot_regular_files(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut snapshot = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = std::fs::symlink_metadata(&path).unwrap();
            assert!(!a3s_use_core::metadata_is_link_or_reparse_point(&metadata));
            if metadata.is_dir() {
                stack.push(path);
            } else if metadata.is_file() {
                snapshot.insert(
                    path.strip_prefix(root).unwrap().to_path_buf(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    snapshot
}
