use a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER;
use tempfile::TempDir;

use super::payload_knowledge_tests::support::{control_installation, paths};
use super::payload_owner::*;
use super::ControlStore;
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation, StateRestoreHistoryFixture,
};

pub(in crate::control_store) mod support;

use support::*;

#[test]
fn restore_coordinator_snapshot_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlRestoreCoordinatorSnapshot>();
    assert_send_sync::<VerifiedControlRestoreCoordinatorSnapshot>();
}

#[tokio::test]
async fn snapshot_archives_only_terminal_restore_history() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let first_terminal = restore_history_fixture(&installation, 1_000).await;
    let second_terminal = restore_history_fixture(&installation, 1_100).await;
    let active = restore_history_fixture(&installation, 2_000).await;
    write_restore_history_operation(
        &paths.installation_state_root(),
        &second_terminal.plan_digest,
        &second_terminal.completed_operation,
    );
    write_restore_history_operation(
        &paths.installation_state_root(),
        &first_terminal.plan_digest,
        &first_terminal.completed_operation,
    );
    write_active(&paths.installation_state_root(), &active);

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("restore-history.a3s-use-payload");
    let snapshot = session
        .snapshot_restore_coordinator(archive.clone(), 3_000)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.binding, *session.binding());
    assert_eq!(snapshot.manifest.entries.len(), 2);
    let mut terminal = [first_terminal, second_terminal];
    terminal.sort_by(|left, right| left.plan_digest.cmp(&right.plan_digest));
    assert_eq!(
        snapshot.manifest.entries[0].plan_digest,
        terminal[0].plan_digest
    );
    assert_eq!(
        snapshot.manifest.entries[1].plan_digest,
        terminal[1].plan_digest
    );
    assert_eq!(snapshot.manifest.excluded_active_files, 2);
    assert_eq!(snapshot.receipt.file_count, 2);
    let expected_archive = terminal
        .iter()
        .flat_map(|fixture| fixture.completed_operation.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(snapshot.receipt.byte_count, expected_archive.len() as u64);
    assert_eq!(std::fs::read(&archive).unwrap(), expected_archive);
    assert!(!serde_json::to_string(&snapshot)
        .unwrap()
        .contains(&temporary.path().display().to_string()));

    snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();

    let mut receipts = dummy_receipts(&session, ControlPayloadOwnerId::RestoreCoordinator);
    receipts.push(snapshot.receipt);
    let complete = session.complete(receipts).unwrap();
    assert_eq!(complete.receipts.len(), 5);
}

#[tokio::test]
async fn empty_or_active_only_history_creates_no_archive() {
    for active in ["none", "planned", "marker-only", "completed"] {
        let temporary = TempDir::new().unwrap();
        let installation = control_installation();
        let paths = paths(&temporary, installation.clone());
        let store = ControlStore::from_extension_paths(&paths).unwrap();
        store.initialize().await.unwrap();
        if active != "none" {
            let operation = restore_history_fixture(&installation, 4_000).await;
            match active {
                "planned" => write_active(&paths.installation_state_root(), &operation),
                "marker-only" => std::fs::write(
                    paths
                        .installation_state_root()
                        .join(ACTIVE_STATE_RESTORE_MARKER),
                    &operation.active_marker,
                )
                .unwrap(),
                "completed" => {
                    write_restore_history_operation(
                        &paths.installation_state_root(),
                        &operation.plan_digest,
                        &operation.completed_operation,
                    );
                    std::fs::write(
                        paths
                            .installation_state_root()
                            .join(ACTIVE_STATE_RESTORE_MARKER),
                        &operation.active_marker,
                    )
                    .unwrap();
                }
                _ => unreachable!(),
            }
        }
        let registry = registry();
        let session = store
            .begin_payload_snapshot(registry.clone())
            .await
            .unwrap();
        let archive = temporary.path().join("absent.a3s-use-payload");
        let snapshot = session
            .snapshot_restore_coordinator(archive.clone(), 5_000)
            .await
            .unwrap();

        assert_eq!(
            snapshot.manifest.payload,
            ControlRestoreCoordinatorState::Absent
        );
        assert!(snapshot.manifest.entries.is_empty());
        assert_eq!(
            snapshot.manifest.excluded_active_files,
            match active {
                "none" => 0,
                "marker-only" => 1,
                "planned" | "completed" => 2,
                _ => unreachable!(),
            }
        );
        assert_eq!(snapshot.receipt.file_count, 0);
        assert!(!archive.exists());
        snapshot
            .verify_offline(&registry, session.binding(), session.control_export(), None)
            .await
            .unwrap();
    }
}

#[tokio::test]
async fn offline_verification_rejects_archive_control_and_manifest_tampering() {
    let fixture = snapshot_fixture(6_000).await;
    assert_eq!(
        fixture
            .snapshot
            .verify_offline(
                &fixture.registry,
                fixture.session.binding(),
                b"{}",
                Some(fixture.archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut bytes = std::fs::read(&fixture.archive).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&fixture.archive, bytes).unwrap();
    assert_eq!(
        fixture
            .snapshot
            .verify_offline(
                &fixture.registry,
                fixture.session.binding(),
                fixture.session.control_export(),
                Some(fixture.archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );

    let mut encoded = serde_json::to_value(&fixture.snapshot).unwrap();
    encoded["manifest"]["excludedActiveFiles"] = serde_json::Value::from(1_u64);
    let rebound: ControlRestoreCoordinatorSnapshot = serde_json::from_value(encoded).unwrap();
    assert_eq!(
        rebound
            .verify_offline(
                &fixture.registry,
                fixture.session.binding(),
                fixture.session.control_export(),
                Some(fixture.archive),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
}

fn write_active(state_root: &std::path::Path, fixture: &StateRestoreHistoryFixture) {
    write_restore_history_operation(state_root, &fixture.plan_digest, &fixture.planned_operation);
    std::fs::write(
        state_root.join(ACTIVE_STATE_RESTORE_MARKER),
        &fixture.active_marker,
    )
    .unwrap();
}
