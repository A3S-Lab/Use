use tempfile::TempDir;

use super::super::payload_knowledge_tests::support::{control_installation, paths};
use super::super::payload_owner::*;
use super::super::payload_restore_coordinator_tests::support::registry;
use super::super::ControlStore;
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation, StateRestoreHistoryFixture,
};

pub(in crate::control_store) struct VerifiedRestoreFixture {
    pub(in crate::control_store) _temporary: TempDir,
    pub(in crate::control_store) operations: Vec<StateRestoreHistoryFixture>,
    pub(in crate::control_store) verified: VerifiedControlRestoreCoordinatorSnapshot,
}

pub(in crate::control_store) async fn verified_restore_fixture(
    count: usize,
    started_at_ms: u64,
) -> VerifiedRestoreFixture {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let mut operations = Vec::with_capacity(count);
    for index in 0..count {
        let operation = restore_history_fixture(
            &installation,
            started_at_ms + u64::try_from(index).unwrap() * 10,
        )
        .await;
        write_restore_history_operation(
            &source_paths.installation_state_root(),
            &operation.plan_digest,
            &operation.completed_operation,
        );
        operations.push(operation);
    }
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive_path = temporary.path().join("restore-history.archive");
    let snapshot = session
        .snapshot_restore_coordinator(archive_path.clone(), started_at_ms + 1_000)
        .await
        .unwrap();
    let archive = archive_path.exists().then_some(archive_path);
    let verified = snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            archive.clone(),
        )
        .await
        .unwrap();
    VerifiedRestoreFixture {
        _temporary: temporary,
        operations,
        verified,
    }
}
