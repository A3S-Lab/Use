use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::populated_snapshot;
use super::payload_owner::*;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[tokio::test]
async fn capability_payload_is_snapshotted_staged_and_activated_with_the_complete_set() {
    let verified = populated_snapshot(15_100).await;
    let capability_snapshot = &verified.manifest().capability_payload;
    assert!(matches!(
        capability_snapshot.manifest.payload,
        ControlCapabilityPayloadState::Absent
    ));
    assert!(capability_snapshot.manifest.entries.is_empty());
    assert_eq!(capability_snapshot.receipt.file_count, 0);

    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(staged.capability_payload_candidate_path().is_none());

    let result = staged.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 7);
    assert!(!state_root.join("capability-gateway").exists());
}

#[test]
fn capability_payload_restore_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StagedControlInstallationRestore>();
    assert_send_sync::<ControlCapabilityPayloadEntry>();
    assert_send_sync::<ControlCapabilityPayloadEntryKind>();
    assert_send_sync::<ControlCapabilityPayloadRestoreResult>();
    assert_send_sync::<ControlCapabilityPayloadRestoreState>();
    assert_send_sync::<ControlCapabilityPayloadSnapshot>();
    assert_send_sync::<VerifiedControlCapabilityPayloadSnapshot>();
}
