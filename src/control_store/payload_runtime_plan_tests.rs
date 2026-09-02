use tempfile::TempDir;

use super::payload_installation_restore_staging_tests::populated_snapshot;
use super::payload_owner::*;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[tokio::test]
async fn runtime_plan_payload_is_snapshotted_staged_and_activated_with_the_complete_set() {
    let verified = populated_snapshot(15_000).await;
    let runtime_snapshot = &verified.manifest().runtime_plans;
    assert!(matches!(
        runtime_snapshot.manifest.payload,
        ControlRuntimePlanPayloadState::Archive { .. }
    ));
    assert_eq!(runtime_snapshot.manifest.entries.len(), 1);
    assert_eq!(runtime_snapshot.receipt.file_count, 1);

    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(staged
        .runtime_plan_candidate_path()
        .is_some_and(|path| path.is_dir()));

    let result = staged.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 6);
    let live_root = state_root.join("runtime-plans");
    assert!(live_root.is_dir());
    let records = std::fs::read_dir(&live_root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name.ends_with(".json"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 1);
}

#[test]
fn runtime_plan_restore_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StagedControlInstallationRestore>();
    assert_send_sync::<ControlRuntimePlanPayloadEntry>();
    assert_send_sync::<ControlRuntimePlanPayloadRestoreResult>();
    assert_send_sync::<ControlRuntimePlanPayloadRestoreState>();
    assert_send_sync::<ControlRuntimePlanPayloadSnapshot>();
    assert_send_sync::<VerifiedControlRuntimePlanPayloadSnapshot>();
}
