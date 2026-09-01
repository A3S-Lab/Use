use a3s_use_extension::{StateMaintenanceLock, ACTIVE_STATE_RESTORE_MARKER};
use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::{absent_snapshot, populated_snapshot};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

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
        .reopen_control_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
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
        .reopen_control_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
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
        .reopen_control_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
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
        .reopen_control_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
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
        .reopen_control_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
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
        .reopen_control_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
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
        .reopen_control_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(reopened.control_candidate_path().is_file());
    assert!(reopened.host_projection_candidate_path().is_some());
    assert!(reopened.knowledge_candidate_path().is_some());
    assert!(reopened.observation_candidate_path().is_some());
    assert!(reopened.restore_coordinator_candidate_path().is_some());
    reopened.activate_control().await.unwrap();
}

#[test]
fn complete_restore_activation_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<super::payload_owner::StagedControlInstallationRestore>();
}
