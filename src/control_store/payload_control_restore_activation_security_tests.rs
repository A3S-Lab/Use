use std::io::Write;

use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::{absent_snapshot, candidate_bytes};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

const ERROR_CODE: &str = "use.control_store.complete_restore_staging_invalid";

#[tokio::test]
async fn control_restore_activation_rejects_a_guard_for_another_root() {
    let verified = absent_snapshot(13_200).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let other = target.path().join("other");
    let wrong = StateMaintenanceLock::new(&other)
        .acquire_exclusive()
        .await
        .unwrap();

    let error = staged
        .control_restore_for_test()
        .activate(&wrong)
        .await
        .unwrap_err();
    assert_eq!(error.code, ERROR_CODE);
    assert!(!state_root.join("control.sqlite3").exists());
    assert!(staged.control_candidate_path().is_file());
}

#[tokio::test]
async fn control_restore_activation_rejects_a_preexisting_live_target_without_clobber() {
    let verified = absent_snapshot(13_300).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let candidate = staged.control_candidate_path().to_path_buf();
    let before = candidate_bytes(candidate.clone());
    let live = state_root.join("control.sqlite3");
    std::fs::write(&live, b"foreign").unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert_eq!(std::fs::read(live).unwrap(), b"foreign");
    assert_eq!(candidate_bytes(candidate), before);
}

#[tokio::test]
async fn control_restore_activation_rejects_candidate_drift() {
    let verified = absent_snapshot(13_400).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(staged.control_candidate_path())
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn control_restore_activation_rejects_a_missing_candidate_without_a_live_database() {
    let verified = absent_snapshot(13_450).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::remove_file(staged.control_candidate_path()).unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn control_restore_activation_rejects_evidence_drift() {
    let verified = absent_snapshot(13_500).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(
            staged
                .staging_directory()
                .join("control")
                .join("candidate.json"),
        )
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(!state_root.join("control.sqlite3").exists());
}

#[tokio::test]
async fn control_restore_activation_rejects_attempt_descriptor_drift() {
    let verified = absent_snapshot(13_550).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(staged.staging_directory().join("attempt.json"))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(!state_root.join("control.sqlite3").exists());
    assert!(staged.control_candidate_path().is_file());
}

#[tokio::test]
async fn control_restore_activation_rejects_completed_live_drift() {
    let verified = absent_snapshot(13_600).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    staged.activate_control().await.unwrap();
    std::fs::OpenOptions::new()
        .append(true)
        .open(state_root.join("control.sqlite3"))
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
}

#[tokio::test]
async fn control_restore_activation_rejects_an_operational_sidecar() {
    let verified = absent_snapshot(13_700).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    std::fs::write(state_root.join("control.sqlite3-wal"), b"foreign").unwrap();

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert!(!state_root.join("control.sqlite3").exists());
    assert!(staged.control_candidate_path().is_file());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn control_restore_activation_rejects_a_linked_live_target() {
    let verified = absent_snapshot(13_800).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let external = target.path().join("external");
    std::fs::create_dir(&external).unwrap();
    std::fs::write(external.join("sentinel"), b"outside").unwrap();
    crate::test_filesystem::create_directory_link(&external, &state_root.join("control.sqlite3"));

    assert_eq!(
        staged.activate_control().await.unwrap_err().code,
        ERROR_CODE
    );
    assert_eq!(
        std::fs::read(external.join("sentinel")).unwrap(),
        b"outside"
    );
    assert!(staged.control_candidate_path().is_file());
}
