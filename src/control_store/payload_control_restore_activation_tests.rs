use tempfile::TempDir;

use super::aggregate_tests::fixtures::control_installation;
use super::payload_installation_restore_staging_tests::absent_snapshot;
use super::payload_installation_snapshot_tests::registry;
use super::ControlStore;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[tokio::test]
async fn control_restore_activation_publishes_and_replays_one_exact_database() {
    let verified = absent_snapshot(13_000).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let candidate = staged.control_candidate_path().to_path_buf();

    let first = staged.activate_control().await.unwrap();
    first.validate(&registry()).unwrap();
    let encoded = serde_json::to_vec(&first).unwrap();
    let decoded: super::payload_owner::ControlStoreRestoreResult =
        serde_json::from_slice(&encoded).unwrap();
    assert_eq!(decoded, first);
    assert!(!String::from_utf8(encoded)
        .unwrap()
        .contains(&state_root.display().to_string()));
    assert!(!candidate.exists());
    assert!(state_root.join("control.sqlite3").is_file());

    let replay = staged.activate_control().await.unwrap();
    assert_eq!(replay, first);
    drop(staged);

    let restored = ControlStore::new(state_root, control_installation()).unwrap();
    restored.inspect().await.unwrap();
}

#[tokio::test]
async fn control_restore_activation_reconciles_the_post_publication_boundary() {
    let verified = absent_snapshot(13_100).await;
    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let candidate = staged.control_candidate_path().to_path_buf();
    let live = state_root.join("control.sqlite3");
    std::fs::rename(&candidate, &live).unwrap();

    let result = staged.activate_control().await.unwrap();
    result.validate(&registry()).unwrap();
    assert!(!candidate.exists());
    assert!(live.is_file());
}

#[test]
fn control_restore_result_contract_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<super::payload_owner::ControlStoreRestoreResult>();
}
