use a3s_use_extension::{StateMaintenanceLock, ACTIVE_STATE_RESTORE_MARKER};
use tempfile::TempDir;

use super::payload_knowledge_tests::support::{control_installation, paths};
use super::payload_owner::*;
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation, StateRestoreHistoryFixture,
};

pub(in crate::control_store) mod support;

use support::*;

#[test]
fn restore_coordinator_restore_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StagedControlRestoreCoordinatorRestore>();
    assert_send_sync::<ControlRestoreCoordinatorRestoreResult>();
}

#[tokio::test]
async fn activation_replaces_terminal_history_and_preserves_the_active_restore() {
    let source = verified_restore_fixture(2, 20_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let prior = restore_history_fixture(&installation, 21_000).await;
    write_restore_history_operation(
        &target_paths.installation_state_root(),
        &prior.plan_digest,
        &prior.completed_operation,
    );
    let staging = target_paths
        .installation_state_root()
        .join("control-restore-coordinator-staging");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    assert!(staged.candidate_path().is_some());

    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 22_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );
    let marker_before = std::fs::read(
        target_paths
            .installation_state_root()
            .join(ACTIVE_STATE_RESTORE_MARKER),
    )
    .unwrap();

    let result = staged.activate(&maintenance).await.unwrap();
    assert_eq!(result.active_plan_digest, active.plan_digest);
    assert_eq!(result.pruned_source_plan_digest, None);
    assert!(matches!(
        result.payload,
        ControlRestoreCoordinatorRestoreState::Archive {
            source_terminal_records: 2,
            restored_terminal_records: 2,
            ..
        }
    ));
    assert_eq!(
        std::fs::read(
            target_paths
                .installation_state_root()
                .join(ACTIVE_STATE_RESTORE_MARKER)
        )
        .unwrap(),
        marker_before
    );
    assert_eq!(
        std::fs::read(operation_path(
            &target_paths.installation_state_root(),
            &active.plan_digest,
        ))
        .unwrap(),
        active.planned_operation
    );
    assert!(!operation_path(&target_paths.installation_state_root(), &prior.plan_digest,).exists());
    for operation in &source.operations {
        assert_eq!(
            std::fs::read(operation_path(
                &target_paths.installation_state_root(),
                &operation.plan_digest,
            ))
            .unwrap(),
            operation.completed_operation
        );
    }

    assert_eq!(staged.activate(&maintenance).await.unwrap(), result);
    std::fs::write(
        operation_path(&target_paths.installation_state_root(), &active.plan_digest),
        &active.completed_operation,
    )
    .unwrap();
    let reopened = source
        .verified
        .reopen_staged_restore(
            target_paths.installation_state_root(),
            staging,
            &maintenance,
        )
        .await
        .unwrap();
    assert_eq!(reopened.activate(&maintenance).await.unwrap(), result);
}

#[tokio::test]
async fn absent_snapshot_retires_history_but_preserves_marker_only_handoff() {
    let source = verified_restore_fixture(0, 23_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let prior = restore_history_fixture(&installation, 24_000).await;
    write_restore_history_operation(
        &target_paths.installation_state_root(),
        &prior.plan_digest,
        &prior.completed_operation,
    );
    let staging = target_paths
        .installation_state_root()
        .join("empty-coordinator-staging");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging)
        .await
        .unwrap();
    assert!(staged.candidate_path().is_none());
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 25_000).await;
    std::fs::write(
        target_paths
            .installation_state_root()
            .join(ACTIVE_STATE_RESTORE_MARKER),
        &active.active_marker,
    )
    .unwrap();

    let result = staged.activate(&maintenance).await.unwrap();
    assert_eq!(result.active_plan_digest, active.plan_digest);
    assert_eq!(
        result.payload,
        ControlRestoreCoordinatorRestoreState::Absent
    );
    assert!(!operation_path(&target_paths.installation_state_root(), &prior.plan_digest,).exists());
    assert!(target_paths
        .installation_state_root()
        .join(ACTIVE_STATE_RESTORE_MARKER)
        .exists());

    write_restore_history_operation(
        &target_paths.installation_state_root(),
        &active.plan_digest,
        &active.planned_operation,
    );
    assert_eq!(staged.activate(&maintenance).await.unwrap(), result);
}

#[tokio::test]
async fn activation_recovers_a_complete_publication_partial() {
    let source = verified_restore_fixture(2, 26_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let prior = restore_history_fixture(&installation, 27_000).await;
    write_restore_history_operation(
        &target_paths.installation_state_root(),
        &prior.plan_digest,
        &prior.completed_operation,
    );
    let staging = target_paths
        .installation_state_root()
        .join("publication-replay-coordinator-staging");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging.clone())
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 28_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );
    let result = staged.activate(&maintenance).await.unwrap();

    let operation = &source.operations[0];
    let live_operation = operation_path(
        &target_paths.installation_state_root(),
        &operation.plan_digest,
    );
    let live_directory = live_operation.parent().unwrap().to_path_buf();
    let publishing_root = staging.join("publishing");
    std::fs::create_dir_all(&publishing_root).unwrap();
    let publishing_directory =
        publishing_root.join(operation.plan_digest.strip_prefix("sha256:").unwrap());
    std::fs::rename(&live_directory, &publishing_directory).unwrap();
    std::fs::rename(
        publishing_directory.join("operation.json"),
        publishing_directory.join("operation.json.partial"),
    )
    .unwrap();

    assert_eq!(staged.activate(&maintenance).await.unwrap(), result);
    assert_eq!(
        std::fs::read(live_operation).unwrap(),
        operation.completed_operation
    );
    assert_eq!(std::fs::read_dir(publishing_root).unwrap().count(), 0);
}

#[tokio::test]
async fn full_source_history_prunes_its_native_oldest_for_the_active_restore() {
    let source = verified_restore_fixture(64, 30_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation.clone());
    std::fs::create_dir_all(target_paths.installation_state_root()).unwrap();
    let staging = target_paths
        .installation_state_root()
        .join("capacity-coordinator-staging");
    let staged = source
        .verified
        .stage_restore(target_paths.installation_state_root(), staging)
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let active = restore_history_fixture(&installation, 40_000).await;
    write_active(
        &target_paths.installation_state_root(),
        &active,
        &active.planned_operation,
    );

    let result = staged.activate(&maintenance).await.unwrap();
    assert_eq!(
        result.pruned_source_plan_digest.as_deref(),
        Some(source.operations[0].plan_digest.as_str())
    );
    assert!(matches!(
        result.payload,
        ControlRestoreCoordinatorRestoreState::Archive {
            source_terminal_records: 64,
            restored_terminal_records: 63,
            ..
        }
    ));
    assert!(!operation_path(
        &target_paths.installation_state_root(),
        &source.operations[0].plan_digest,
    )
    .exists());
}

#[tokio::test]
async fn complete_restore_marker_does_not_reserve_a_terminal_history_slot() {
    let source = verified_restore_fixture(64, 41_000).await;
    let target = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&target, installation);
    let state_root = target_paths.installation_state_root();
    std::fs::create_dir_all(&state_root).unwrap();
    let staged = source
        .verified
        .stage_restore(&state_root, state_root.join("complete-capacity-staging"))
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(&state_root)
        .acquire_exclusive()
        .await
        .unwrap();
    let plan_digest = format!("sha256:{}", "a".repeat(64));
    let marker = crate::state_restore::ControlInstallationRestoreActiveMarker::new(
        &plan_digest,
        &format!("sha256:{}", "b".repeat(64)),
    )
    .unwrap()
    .canonical_bytes()
    .unwrap();
    std::fs::write(state_root.join(ACTIVE_STATE_RESTORE_MARKER), &marker).unwrap();

    let result = staged
        .activate_for_complete_restore(&maintenance, &plan_digest, &marker)
        .await
        .unwrap();
    assert_eq!(result.pruned_source_plan_digest, None);
    assert!(matches!(
        result.payload,
        ControlRestoreCoordinatorRestoreState::Archive {
            source_terminal_records: 64,
            restored_terminal_records: 64,
            ..
        }
    ));
    for operation in &source.operations {
        assert!(operation_path(&state_root, &operation.plan_digest).is_file());
    }
}

pub(in crate::control_store) fn write_active(
    state_root: &std::path::Path,
    fixture: &StateRestoreHistoryFixture,
    operation: &[u8],
) {
    write_restore_history_operation(state_root, &fixture.plan_digest, operation);
    std::fs::write(
        state_root.join(ACTIVE_STATE_RESTORE_MARKER),
        &fixture.active_marker,
    )
    .unwrap();
}

pub(in crate::control_store) fn operation_path(
    state_root: &std::path::Path,
    plan_digest: &str,
) -> std::path::PathBuf {
    state_root
        .join("operations/state-restores")
        .join(plan_digest.strip_prefix("sha256:").unwrap())
        .join("operation.json")
}
