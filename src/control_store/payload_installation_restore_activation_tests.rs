use std::path::{Path, PathBuf};

use a3s_use_extension::{StateMaintenanceLock, ACTIVE_STATE_RESTORE_MARKER};
use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::{
    absent_snapshot, absent_snapshot_at, populated_snapshot,
};
use super::payload_installation_snapshot_tests::registry;
use super::payload_owner::VerifiedControlInstallationSnapshot;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

const COMPLETE_RESTORE_CHILD_ROOT_ENV: &str = "A3S_USE_TEST_CONTROL_COMPLETE_RESTORE_ROOT";
const COMPLETE_RESTORE_CHILD_ARCHIVE_ENV: &str = "A3S_USE_TEST_CONTROL_COMPLETE_RESTORE_ARCHIVE";
const COMPLETE_RESTORE_CRASH_CHECKPOINT_ENV: &str =
    "A3S_USE_TEST_CONTROL_COMPLETE_RESTORE_CHECKPOINT";
const COMPLETE_RESTORE_CRASH_EXIT_CODE: i32 = 88;

#[tokio::test]
async fn control_checkpoint_survives_guard_release_and_reopens_exactly() {
    let verified = absent_snapshot(14_000).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();

    let first = staged.activate_control().await.unwrap();
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert!(staged.activation_journal_path_for_test().is_file());
    assert_eq!(
        staged.activation_checkpoint_count_for_test().await.unwrap(),
        1
    );
    drop(staged);

    let blocked = StateMaintenanceLock::new(&state_root)
        .try_acquire_shared()
        .await
        .unwrap_err();
    assert_eq!(blocked.code, "use.state.maintenance_restore_active");

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let replay = reopened.activate_control().await.unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        reopened
            .activation_checkpoint_count_for_test()
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn activation_reopens_before_control_publication() {
    let verified = absent_snapshot(14_100).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    assert!(staged.control_candidate_path().is_file());
    assert!(!state_root.join("control.sqlite3").exists());
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    reopened.activate_control().await.unwrap();
    assert!(state_root.join("control.sqlite3").is_file());
    assert_eq!(
        reopened
            .activation_checkpoint_count_for_test()
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn activation_reconciles_publication_before_checkpoint_after_reopen() {
    let verified = absent_snapshot(14_200).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::rename(
        staged.control_candidate_path(),
        state_root.join("control.sqlite3"),
    )
    .unwrap();
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    reopened.activate_control().await.unwrap();
    assert_eq!(
        reopened
            .activation_checkpoint_count_for_test()
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn journal_only_boundary_republishes_the_exact_marker_before_control() {
    let verified = absent_snapshot(14_300).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::remove_file(state_root.join(ACTIVE_STATE_RESTORE_MARKER)).unwrap();
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    reopened.activate_control().await.unwrap();
}

#[tokio::test]
async fn partial_marker_boundary_publishes_the_exact_marker_after_reopen() {
    let verified = absent_snapshot(14_325).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::rename(
        state_root.join(ACTIVE_STATE_RESTORE_MARKER),
        state_root.join(".maintenance.restore.json.partial"),
    )
    .unwrap();
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert!(!state_root
        .join(".maintenance.restore.json.partial")
        .exists());
    reopened.activate_control().await.unwrap();
}

#[tokio::test]
async fn temporary_journal_boundary_publishes_journal_and_marker_after_reopen() {
    let verified = absent_snapshot(14_340).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::remove_file(state_root.join(ACTIVE_STATE_RESTORE_MARKER)).unwrap();
    let journal = staged.activation_journal_path_for_test();
    std::fs::rename(
        &journal,
        staged.staging_directory().join("activation.json.tmp"),
    )
    .unwrap();
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(journal.is_file());
    assert!(!reopened
        .staging_directory()
        .join("activation.json.tmp")
        .exists());
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    reopened.activate_control().await.unwrap();
}

#[tokio::test]
async fn activation_reopens_every_present_unactivated_owner_candidate() {
    let verified = populated_snapshot(14_350).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(reopened.control_candidate_path().is_file());
    assert!(reopened.host_projection_candidate_path().is_some());
    assert!(reopened.runtime_plan_candidate_path().is_some());
    assert!(reopened.knowledge_candidate_path().is_some());
    assert!(reopened.observation_candidate_path().is_some());
    assert!(reopened.restore_coordinator_candidate_path().is_some());
    reopened.activate_control().await.unwrap();
}

#[tokio::test]
async fn complete_activation_checkpoints_every_owner_and_retires_the_marker() {
    let verified = populated_snapshot(14_360).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();

    let first = staged.activate().await.unwrap();
    assert_eq!(first.checkpoint_count_for_test(), 6);
    assert_eq!(
        staged.activation_checkpoint_count_for_test().await.unwrap(),
        6
    );
    assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
    assert!(state_root.join("control.sqlite3").is_file());
    assert!(state_root.join("plugin-host-manager").is_dir());
    assert!(state_root.join("runtime-plans").is_dir());
    assert!(state_root.join("knowledge").is_dir());
    assert!(state_root.join("operations").is_dir());
    assert_eq!(
        terminal_attempt_entries(&state_root),
        ["activation.json", "attempt.json"]
    );
    drop(staged);

    assert!(StateMaintenanceLock::new(&state_root)
        .try_acquire_shared()
        .await
        .unwrap()
        .is_some());
    let journal_before = std::fs::read(
        state_root
            .join(".control-installation-restore")
            .join("activation.json"),
    )
    .unwrap();
    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let replay = reopened.activate().await.unwrap();
    assert_eq!(replay, first);
    assert_eq!(
        std::fs::read(reopened.activation_journal_path_for_test()).unwrap(),
        journal_before
    );
    assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
}

#[tokio::test]
async fn absent_complete_activation_retires_without_inventing_payload_roots() {
    let verified = absent_snapshot(14_370).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();

    let result = staged.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 6);
    assert!(state_root.join("control.sqlite3").is_file());
    assert!(!state_root.join("plugin-host-manager").exists());
    assert!(!state_root.join("knowledge").exists());
    assert!(!state_root.join("operations").exists());
    assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
}

#[tokio::test]
async fn every_owner_effect_before_checkpoint_converges_after_reopen() {
    for completed_prefix in 0..6 {
        let verified = populated_snapshot(14_400 + completed_prefix as u64);
        let verified = verified.await;
        let target = TempDir::new().unwrap();
        let state_root = target.path().join("state");
        let staged = verified
            .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap();
        for _ in 0..completed_prefix {
            staged.activate_next_for_test().await.unwrap();
        }
        staged
            .activate_next_effect_without_checkpoint_for_test()
            .await
            .unwrap();
        assert_eq!(
            staged.activation_checkpoint_count_for_test().await.unwrap(),
            completed_prefix
        );
        drop(staged);

        let reopened = verified
            .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap();
        let result = reopened.activate().await.unwrap();
        assert_eq!(result.checkpoint_count_for_test(), 6);
        assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
    }
}

#[tokio::test]
async fn final_checkpoint_before_marker_retirement_converges_after_reopen() {
    let verified = populated_snapshot(14_500).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    for _ in 0..6 {
        staged.activate_next_for_test().await.unwrap();
    }
    assert_eq!(
        staged.activation_checkpoint_count_for_test().await.unwrap(),
        6
    );
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    drop(staged);

    let reopened = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let result = reopened.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 6);
    assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
}

#[tokio::test]
async fn every_complete_restore_checkpoint_recovers_after_process_exit() {
    let temporary = TempDir::new().unwrap();
    let archive = temporary.path().join("complete-restore.snapshot");
    drop(absent_snapshot_at(archive.clone(), 14_600).await);

    for checkpoint in [
        "journal-published",
        "marker-published",
        "control-store-effect",
        "control-store-checkpoint",
        "runtime-plans-effect",
        "runtime-plans-checkpoint",
        "host-projection-effect",
        "host-projection-checkpoint",
        "knowledge-effect",
        "knowledge-checkpoint",
        "observations-effect",
        "observations-checkpoint",
        "restore-coordinator-effect",
        "restore-coordinator-checkpoint",
        "marker-retired",
        "control-store-staging-retired",
        "runtime-plans-staging-retired",
        "host-projection-staging-retired",
        "knowledge-staging-retired",
        "observations-staging-retired",
        "restore-coordinator-staging-retired",
    ] {
        let state_root = temporary.path().join(format!("state-{checkpoint}"));
        let output = tokio::process::Command::new(std::env::current_exe().unwrap())
            .arg("complete_restore_checkpoint_crash_child")
            .arg("--ignored")
            .arg("--nocapture")
            .arg("--test-threads=1")
            .env(COMPLETE_RESTORE_CHILD_ROOT_ENV, &state_root)
            .env(COMPLETE_RESTORE_CHILD_ARCHIVE_ENV, &archive)
            .env(COMPLETE_RESTORE_CRASH_CHECKPOINT_ENV, checkpoint)
            .output()
            .await
            .unwrap();
        assert_eq!(
            output.status.code(),
            Some(COMPLETE_RESTORE_CRASH_EXIT_CODE),
            "complete restore child did not exit at {checkpoint}: status={:?}, stdout={}, stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let verified =
            VerifiedControlInstallationSnapshot::verify_offline(registry(), archive.clone())
                .await
                .unwrap();
        let reopened = verified
            .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_or_else(|error| panic!("failed to reopen {checkpoint}: {error}"));
        let result = reopened
            .activate()
            .await
            .unwrap_or_else(|error| panic!("failed to recover {checkpoint}: {error}"));
        assert_eq!(result.checkpoint_count_for_test(), 6);
        assert!(state_root.join("control.sqlite3").is_file());
        assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
        assert_eq!(
            terminal_attempt_entries(&state_root),
            ["activation.json", "attempt.json"]
        );
    }
}

fn terminal_attempt_entries(state_root: &Path) -> Vec<String> {
    let mut entries = std::fs::read_dir(state_root.join(".control-installation-restore"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect::<Vec<_>>();
    entries.sort();
    entries
}

#[tokio::test]
#[ignore]
async fn complete_restore_checkpoint_crash_child() {
    let Some(state_root) = std::env::var_os(COMPLETE_RESTORE_CHILD_ROOT_ENV).map(PathBuf::from)
    else {
        return;
    };
    let archive = PathBuf::from(std::env::var_os(COMPLETE_RESTORE_CHILD_ARCHIVE_ENV).unwrap());
    let verified = VerifiedControlInstallationSnapshot::verify_offline(registry(), archive)
        .await
        .unwrap();
    let staged = verified
        .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate().await.unwrap();
}

#[test]
fn complete_restore_activation_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<super::payload_owner::StagedControlInstallationRestore>();
}
