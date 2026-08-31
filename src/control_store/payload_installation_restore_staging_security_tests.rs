use std::io::Write;

use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::{
    absent_snapshot, candidate_bytes, populated_snapshot,
};
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[tokio::test]
async fn complete_restore_rejects_a_nonempty_target_without_touching_it() {
    let verified = absent_snapshot(12_000).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    std::fs::create_dir_all(&state_root).unwrap();
    let sentinel = state_root.join("foreign-authority.json");
    std::fs::write(&sentinel, b"sentinel").unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"sentinel");
    assert!(!state_root.join(".control-installation-restore").exists());
}

#[tokio::test]
async fn complete_restore_rejects_an_attempt_rebound_to_another_snapshot() {
    let first = absent_snapshot(12_100).await;
    let second = absent_snapshot(12_101).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = first
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let control = staged.control_candidate_path().to_path_buf();
    let before = candidate_bytes(control.clone());
    drop(staged);

    assert_eq!(
        second
            .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
    assert_eq!(candidate_bytes(control), before);
}

#[tokio::test]
async fn complete_restore_rejects_completed_control_candidate_tampering() {
    let verified = absent_snapshot(12_200).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let candidate = staged.control_candidate_path().to_path_buf();
    drop(staged);
    std::fs::OpenOptions::new()
        .append(true)
        .open(&candidate)
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
}

#[tokio::test]
async fn complete_restore_rejects_attempt_descriptor_tampering() {
    let verified = absent_snapshot(12_250).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let descriptor = staged.staging_directory().join("attempt.json");
    drop(staged);
    std::fs::OpenOptions::new()
        .append(true)
        .open(descriptor)
        .unwrap()
        .write_all(b"tamper")
        .unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
}

#[tokio::test]
async fn complete_restore_rejects_unknown_attempt_entries() {
    let verified = absent_snapshot(12_275).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let unknown = staged.staging_directory().join("foreign-candidate");
    drop(staged);
    std::fs::write(&unknown, b"foreign").unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
    assert_eq!(std::fs::read(unknown).unwrap(), b"foreign");
}

#[tokio::test]
async fn complete_restore_rejects_storage_policy_rebinding() {
    let verified = absent_snapshot(12_300).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    drop(staged);
    let policy = OkfKnowledgeStoragePolicy::new(1024, 4, 2, 4).unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(state_root, policy)
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
}

#[tokio::test]
async fn complete_restore_rejects_an_incompatible_policy_before_touching_the_target() {
    let verified = populated_snapshot(12_350).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let incompatible = OkfKnowledgeStoragePolicy::new(1024, 4, 2, 4).unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(state_root.clone(), incompatible)
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
    assert!(!state_root.exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn complete_restore_rejects_a_linked_attempt_root() {
    let verified = absent_snapshot(12_400).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let external = target.path().join("external");
    std::fs::create_dir_all(&state_root).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    crate::test_filesystem::create_directory_link(
        &external,
        &state_root.join(".control-installation-restore"),
    );

    assert_eq!(
        verified
            .stage_clean_restore(state_root, OkfKnowledgeStoragePolicy::default())
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_restore_staging_invalid"
    );
    assert!(std::fs::read_dir(external).unwrap().next().is_none());
}
