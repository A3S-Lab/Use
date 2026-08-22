use std::path::{Path, PathBuf};

use super::super::journal::MAX_OPERATION_COUNT;
use super::super::*;
use super::fixture_paths;

struct RequiredRestoreFixture {
    paths: ExtensionPaths,
    manager: StateRestoreManager,
    backup_path: PathBuf,
    rollback_path: PathBuf,
    file: PathBuf,
    plan: StateRestorePlan,
    plan_digest: String,
}

async fn required_restore_fixture(root: &Path) -> RequiredRestoreFixture {
    let paths = fixture_paths(root);
    let file = paths.state_root().join("knowledge/value.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"candidate").unwrap();
    let backup_path = root.join("candidate.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup_path)
        .await
        .unwrap();
    std::fs::write(&file, b"reviewed live").unwrap();
    let manager = StateRestoreManager::new(paths.clone());
    let plan = manager.plan_restore(&backup_path).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    RequiredRestoreFixture {
        paths,
        manager,
        backup_path,
        rollback_path: root.join("rollback.a3s-use-state-backup"),
        file,
        plan,
        plan_digest,
    }
}

async fn begin_operation(fixture: &RequiredRestoreFixture) -> StateRestoreOperation {
    let rollback = StateBackupManager::new(fixture.paths.clone())
        .backup(&fixture.rollback_path)
        .await
        .unwrap();
    let operation = StateRestoreOperation::new(
        fixture.plan.clone(),
        fixture.plan_digest.clone(),
        rollback.descriptor_digest().unwrap(),
        now_ms().unwrap(),
    )
    .unwrap();
    fixture
        .manager
        .operations
        .activate(&operation)
        .await
        .unwrap();
    fixture.manager.operations.begin(&operation).await.unwrap();
    operation
}

#[tokio::test]
async fn apply_rejects_a_mismatched_existing_rollback_before_mutation() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = required_restore_fixture(temporary.path()).await;
    let candidate_archive = std::fs::read(&fixture.backup_path).unwrap();
    std::fs::write(&fixture.rollback_path, &candidate_archive).unwrap();

    let error = fixture
        .manager
        .apply_restore(
            &fixture.backup_path,
            &fixture.rollback_path,
            &fixture.plan_digest,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.state_restore_rollback_mismatch");
    assert_eq!(std::fs::read(&fixture.file).unwrap(), b"reviewed live");
    assert_eq!(
        std::fs::read(&fixture.rollback_path).unwrap(),
        candidate_archive
    );
    assert!(!fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER)
        .exists());
}

#[tokio::test]
async fn validly_encoded_marker_and_operation_tampering_fail_closed() {
    let marker_case = tempfile::tempdir().unwrap();
    let marker_fixture = required_restore_fixture(marker_case.path()).await;
    begin_operation(&marker_fixture).await;
    let marker_path = marker_fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER);
    let mut marker: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&marker_path).unwrap()).unwrap();
    marker["operationDigest"] = serde_json::json!(format!("sha256:{}", "0".repeat(64)));
    let marker_bytes = serde_json::to_vec(&marker).unwrap();
    std::fs::write(&marker_path, &marker_bytes).unwrap();

    let error = marker_fixture
        .manager
        .apply_restore(
            &marker_fixture.backup_path,
            &marker_fixture.rollback_path,
            &marker_fixture.plan_digest,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_restore_operation_conflict");
    assert_eq!(std::fs::read(&marker_path).unwrap(), marker_bytes);
    assert_eq!(
        std::fs::read(&marker_fixture.file).unwrap(),
        b"reviewed live"
    );

    let operation_case = tempfile::tempdir().unwrap();
    let operation_fixture = required_restore_fixture(operation_case.path()).await;
    let mut operation = begin_operation(&operation_fixture).await;
    operation.started_at_ms += 1;
    operation.validate().unwrap();
    let digest = operation_fixture
        .plan_digest
        .strip_prefix("sha256:")
        .unwrap();
    let journal_path = operation_fixture
        .paths
        .state_root()
        .join("operations/state-restores")
        .join(digest)
        .join("operation.json");
    let journal_bytes = serde_json::to_vec(&operation).unwrap();
    std::fs::write(&journal_path, &journal_bytes).unwrap();

    let error = operation_fixture
        .manager
        .apply_restore(
            &operation_fixture.backup_path,
            &operation_fixture.rollback_path,
            &operation_fixture.plan_digest,
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.state_restore_operation_conflict");
    assert_eq!(std::fs::read(&journal_path).unwrap(), journal_bytes);
    assert_eq!(
        std::fs::read(&operation_fixture.file).unwrap(),
        b"reviewed live"
    );
}

#[tokio::test]
async fn whole_install_restore_never_takes_over_a_standalone_knowledge_marker() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = required_restore_fixture(temporary.path()).await;
    let marker_path = fixture
        .paths
        .state_root()
        .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER);
    let foreign_marker = serde_json::to_vec(&serde_json::json!({
        "schema": "a3s.use.active-state-restore.v2",
        "scope": { "kind": "workspace", "id": "fixture" },
        "planDigest": format!("sha256:{}", "1".repeat(64)),
        "operation": {}
    }))
    .unwrap();
    std::fs::write(&marker_path, &foreign_marker).unwrap();

    let error = fixture
        .manager
        .apply_restore(
            &fixture.backup_path,
            &fixture.rollback_path,
            &fixture.plan_digest,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.state_restore_operation_invalid");
    assert_eq!(std::fs::read(&marker_path).unwrap(), foreign_marker);
    assert_eq!(std::fs::read(&fixture.file).unwrap(), b"reviewed live");
    assert!(!fixture.rollback_path.exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn publication_rejects_candidate_root_link_or_reparse_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = required_restore_fixture(temporary.path()).await;
    let mut operation = begin_operation(&fixture).await;
    filesystem::stage_candidates(&fixture.paths, &fixture.backup_path, &operation)
        .await
        .unwrap();
    operation
        .advance(StateRestoreOperationStatus::Staged, None)
        .unwrap();
    fixture.manager.operations.save(&operation).await.unwrap();

    let segment = fixture.plan_digest.strip_prefix("sha256:").unwrap();
    let candidate_root = fixture
        .paths
        .state_root()
        .join(format!(".state-restore-{segment}"));
    std::fs::remove_dir_all(&candidate_root).unwrap();
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"do not modify").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, &candidate_root).unwrap();
    #[cfg(windows)]
    {
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&candidate_root)
            .arg(&outside)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let error = fixture
        .manager
        .apply_restore(
            &fixture.backup_path,
            &fixture.rollback_path,
            &fixture.plan_digest,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.state_backup_path_invalid");
    assert_eq!(std::fs::read(&fixture.file).unwrap(), b"reviewed live");
    assert_eq!(
        std::fs::read(outside.join("sentinel")).unwrap(),
        b"do not modify"
    );
}

#[tokio::test]
async fn sixty_fifth_operation_recovers_interrupted_oldest_terminal_pruning() {
    let temporary = tempfile::tempdir().unwrap();
    let fixture = required_restore_fixture(temporary.path()).await;
    let plans = distinct_plans(&fixture.plan, MAX_OPERATION_COUNT + 1);
    let root = fixture.paths.state_root().join("operations/state-restores");
    std::fs::create_dir_all(&root).unwrap();
    let mut completed = Vec::new();
    for (index, plan) in plans.iter().take(MAX_OPERATION_COUNT).cloned().enumerate() {
        let operation = completed_operation(plan, index as u64 + 1);
        let segment = operation.plan_digest.strip_prefix("sha256:").unwrap();
        let directory = root.join(segment);
        std::fs::create_dir(&directory).unwrap();
        std::fs::write(
            directory.join("operation.json"),
            serde_json::to_vec(&operation).unwrap(),
        )
        .unwrap();
        completed.push(operation);
    }
    let oldest = &completed[0];
    let oldest_segment = oldest.plan_digest.strip_prefix("sha256:").unwrap();
    let tombstone = root.join(format!(".pruning-{oldest_segment}"));
    std::fs::rename(root.join(oldest_segment), &tombstone).unwrap();

    let before = fixture.manager.diagnose_restore().await.unwrap();
    assert_eq!(before.retained_operation_directories, MAX_OPERATION_COUNT);
    assert_eq!(before.unrecorded_operation_directories, 1);
    assert_eq!(before.operations.len(), MAX_OPERATION_COUNT - 1);
    assert!(tombstone.exists());

    let next = StateRestoreOperation::new(
        plans[MAX_OPERATION_COUNT].clone(),
        plans[MAX_OPERATION_COUNT].descriptor_digest().unwrap(),
        sha256(b"rollback-65"),
        1_000,
    )
    .unwrap();
    fixture.manager.operations.begin(&next).await.unwrap();

    let after = fixture.manager.diagnose_restore().await.unwrap();
    assert_eq!(after.retained_operation_directories, MAX_OPERATION_COUNT);
    assert_eq!(after.unrecorded_operation_directories, 0);
    assert_eq!(after.operations.len(), MAX_OPERATION_COUNT);
    assert!(!tombstone.exists());
    assert!(!root.join(oldest_segment).exists());
    assert!(root
        .join(next.plan_digest.strip_prefix("sha256:").unwrap())
        .join("operation.json")
        .is_file());
}

fn distinct_plans(base: &StateRestorePlan, count: usize) -> Vec<StateRestorePlan> {
    (0..count)
        .map(|index| {
            let mut plan = base.clone();
            let digest = sha256(format!("candidate-{index:04}").as_bytes());
            let entry = plan
                .backup
                .entries
                .iter_mut()
                .find(|entry| entry.path == "knowledge/value.bin")
                .unwrap();
            entry.sha256 = digest.clone();
            plan.backup.inventory_digest = digest_entries(&plan.backup.entries).unwrap();
            for summary in &mut plan.backup.families {
                let entries = plan
                    .backup
                    .entries
                    .iter()
                    .filter(|entry| entry.family == summary.family)
                    .collect::<Vec<_>>();
                summary.inventory_digest =
                    sha256(&canonical_json(&entries, "test backup family").unwrap());
            }
            plan.backup_manifest_digest = plan.backup.descriptor_digest().unwrap();
            let action = plan
                .actions
                .iter_mut()
                .find(|action| action.path == "knowledge/value.bin")
                .unwrap();
            action.after.as_mut().unwrap().sha256 = digest;
            plan.validate().unwrap();
            plan
        })
        .collect()
}

fn completed_operation(plan: StateRestorePlan, started_at_ms: u64) -> StateRestoreOperation {
    let plan_digest = plan.descriptor_digest().unwrap();
    let mut operation = StateRestoreOperation::new(
        plan,
        plan_digest,
        sha256(format!("rollback-{started_at_ms}").as_bytes()),
        started_at_ms,
    )
    .unwrap();
    for status in [
        StateRestoreOperationStatus::Staged,
        StateRestoreOperationStatus::Publishing,
        StateRestoreOperationStatus::Published,
        StateRestoreOperationStatus::CandidatesRemoved,
        StateRestoreOperationStatus::Verified,
    ] {
        operation.advance(status, None).unwrap();
    }
    operation
        .advance(
            StateRestoreOperationStatus::Completed,
            Some(started_at_ms + 1),
        )
        .unwrap();
    operation
}

#[cfg(windows)]
#[tokio::test]
async fn restore_preserves_windows_read_only_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = fixture_paths(temporary.path());
    let file = paths.state_root().join("knowledge/read-only.bin");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"candidate").unwrap();
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(true);
    std::fs::set_permissions(&file, permissions).unwrap();
    let backup = temporary.path().join("read-only.a3s-use-state-backup");
    StateBackupManager::new(paths.clone())
        .backup(&backup)
        .await
        .unwrap();
    let mut permissions = std::fs::metadata(&file).unwrap().permissions();
    permissions.set_readonly(false);
    std::fs::set_permissions(&file, permissions).unwrap();
    std::fs::write(&file, b"live").unwrap();
    let manager = StateRestoreManager::new(paths);
    let plan = manager.plan_restore(&backup).await.unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    manager
        .apply_restore(
            backup,
            temporary.path().join("rollback.a3s-use-state-backup"),
            &plan_digest,
        )
        .await
        .unwrap();

    assert!(std::fs::metadata(&file).unwrap().permissions().readonly());
    assert_eq!(std::fs::read(file).unwrap(), b"candidate");
}
