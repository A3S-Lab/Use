use std::io::Write;

use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;
use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::{
    absent_snapshot, candidate_bytes, populated_snapshot,
};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

const ERROR_CODE: &str = "use.control_store.complete_restore_activation_invalid";

#[tokio::test]
async fn activation_rejects_marker_tampering_without_publishing_control() {
    let verified = absent_snapshot(14_400).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(state_root.join(ACTIVE_STATE_RESTORE_MARKER))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();
    let candidate = staged.control_candidate_path().to_path_buf();
    let before = candidate_bytes(candidate.clone());

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert_eq!(candidate_bytes(candidate), before);
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn activation_rejects_journal_tampering_without_publishing_control() {
    let verified = absent_snapshot(14_500).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(staged.activation_journal_path_for_test())
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(staged.control_candidate_path().is_file());
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn reopen_rejects_a_missing_marker_after_control_publication() {
    let verified = absent_snapshot(14_600).await;
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
    std::fs::remove_file(state_root.join(ACTIVE_STATE_RESTORE_MARKER)).unwrap();
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
}

#[tokio::test]
async fn reopen_rejects_a_marker_without_its_journal() {
    let verified = absent_snapshot(14_700).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::remove_file(staged.activation_journal_path_for_test()).unwrap();
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
}

#[tokio::test]
async fn activation_rejects_a_foreign_global_marker() {
    let verified = absent_snapshot(14_800).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::write(
        state_root.join(ACTIVE_STATE_RESTORE_MARKER),
        br#"{"schema":"foreign"}"#,
    )
    .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(staged.control_candidate_path().is_file());
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn reopen_rejects_ambiguous_final_and_partial_markers() {
    let verified = absent_snapshot(14_900).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    std::fs::copy(
        state_root.join(ACTIVE_STATE_RESTORE_MARKER),
        state_root.join(".maintenance.restore.json.partial"),
    )
    .unwrap();
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
}

#[tokio::test]
async fn reopen_rejects_rebinding_to_another_verified_snapshot() {
    let original = absent_snapshot(15_000).await;
    let replacement = absent_snapshot(15_001).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = original
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    drop(staged);

    let error = replacement
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
}

#[tokio::test]
async fn reopen_rejects_a_missing_marker_after_control_checkpoint() {
    let verified = absent_snapshot(15_100).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate_control().await.unwrap();
    std::fs::remove_file(state_root.join(ACTIVE_STATE_RESTORE_MARKER)).unwrap();
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
}

#[tokio::test]
async fn activation_revalidates_every_owner_before_control_intent() {
    let verified = populated_snapshot(15_150).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(staged.knowledge_candidate_path().unwrap())
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(staged.control_candidate_path().is_file());
    assert!(!staged.activation_journal_path_for_test().exists());
    assert!(!state_root.join(ACTIVE_STATE_RESTORE_MARKER).exists());
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn reopen_rejects_a_missing_marker_after_a_later_owner_effect() {
    let verified = populated_snapshot(15_175).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate_next_for_test().await.unwrap();
    staged
        .activate_next_effect_without_checkpoint_for_test()
        .await
        .unwrap();
    std::fs::remove_file(state_root.join(ACTIVE_STATE_RESTORE_MARKER)).unwrap();
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
}

#[tokio::test]
async fn reopen_revalidates_complete_live_state_before_marker_retirement() {
    let verified = absent_snapshot(15_180).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    for _ in 0..5 {
        staged.activate_next_for_test().await.unwrap();
    }
    let attempt = state_root.join(".control-installation-restore");
    std::fs::write(state_root.join("control.sqlite3"), b"tamper").unwrap();
    drop(staged);

    let error = verified
        .reopen_activation(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert!(attempt.join("control").is_dir());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn reopen_rejects_a_linked_active_marker() {
    let verified = absent_snapshot(15_200).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    let marker = state_root.join(ACTIVE_STATE_RESTORE_MARKER);
    std::fs::remove_file(&marker).unwrap();
    let outside = target.path().join("outside-marker");
    std::fs::create_dir(&outside).unwrap();
    crate::test_filesystem::create_directory_link(&outside, &marker);
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
    crate::test_filesystem::remove_directory_link(&marker);
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn reopen_rejects_a_linked_activation_journal() {
    let verified = absent_snapshot(15_300).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.begin_control_activation_for_test().await.unwrap();
    let journal = staged.activation_journal_path_for_test();
    std::fs::remove_file(&journal).unwrap();
    let outside = target.path().join("outside-journal");
    std::fs::create_dir(&outside).unwrap();
    crate::test_filesystem::create_directory_link(&outside, &journal);
    drop(staged);

    let error = verified
        .reopen_activation(state_root, OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
    crate::test_filesystem::remove_directory_link(&journal);
}

#[tokio::test]
async fn terminal_retirement_rejects_unknown_attempt_evidence_before_marker_removal() {
    let verified = absent_snapshot(15_400).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    for _ in 0..5 {
        staged.activate_next_for_test().await.unwrap();
    }
    let attempt = state_root.join(".control-installation-restore");
    std::fs::write(attempt.join("foreign.json"), b"foreign").unwrap();

    let error = staged.activate().await.unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert!(attempt.join("control").is_dir());
    assert!(attempt.join("host-projection").is_dir());
    assert!(attempt.join("knowledge").is_dir());
    assert!(attempt.join("observations").is_dir());
    assert!(attempt.join("restore-coordinator").is_dir());
}

#[tokio::test]
async fn terminal_retirement_rejects_attempt_descriptor_drift_before_deletion() {
    let verified = absent_snapshot(15_500).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    for _ in 0..5 {
        staged.activate_next_for_test().await.unwrap();
    }
    let attempt = state_root.join(".control-installation-restore");
    std::fs::OpenOptions::new()
        .append(true)
        .open(attempt.join("attempt.json"))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    let error = staged.activate().await.unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert!(attempt.join("control").is_dir());
    assert!(attempt.join("restore-coordinator").is_dir());
}

#[tokio::test]
async fn terminal_retirement_rejects_missing_staging_before_marker_retirement() {
    let verified = absent_snapshot(15_550).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    for _ in 0..5 {
        staged.activate_next_for_test().await.unwrap();
    }
    let attempt = state_root.join(".control-installation-restore");
    std::fs::remove_dir_all(attempt.join("knowledge")).unwrap();

    let error = staged.activate().await.unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert!(attempt.join("control").is_dir());
    assert!(attempt.join("restore-coordinator").is_dir());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn terminal_retirement_never_traverses_a_linked_staging_tree() {
    let verified = absent_snapshot(15_600).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    for _ in 0..5 {
        staged.activate_next_for_test().await.unwrap();
    }
    let outside = target.path().join("outside-retirement");
    std::fs::create_dir(&outside).unwrap();
    std::fs::write(outside.join("preserved"), b"preserved").unwrap();
    let linked = state_root
        .join(".control-installation-restore")
        .join("linked-retirement");
    crate::test_filesystem::create_directory_link(&outside, &linked);

    let error = staged.activate().await.unwrap_err();

    assert_eq!(error.code, ERROR_CODE);
    assert!(state_root.join(ACTIVE_STATE_RESTORE_MARKER).is_file());
    assert_eq!(
        std::fs::read(outside.join("preserved")).unwrap(),
        b"preserved"
    );
    crate::test_filesystem::remove_directory_link(&linked);
}
