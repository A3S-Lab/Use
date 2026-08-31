use std::path::PathBuf;

use tempfile::TempDir;

use super::super::payload_knowledge_tests::support::{control_installation, paths};
use super::super::payload_owner::*;
use super::super::ControlStore;
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation,
};

pub(in crate::control_store) struct SnapshotFixture {
    pub(in crate::control_store) _temporary: TempDir,
    pub(in crate::control_store) registry: ControlPayloadOwnerRegistry,
    pub(in crate::control_store) session: ControlPayloadSnapshotSession,
    pub(in crate::control_store) snapshot: ControlRestoreCoordinatorSnapshot,
    pub(in crate::control_store) archive: PathBuf,
}

pub(in crate::control_store) async fn snapshot_fixture(started_at_ms: u64) -> SnapshotFixture {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let operation = restore_history_fixture(&installation, started_at_ms).await;
    write_restore_history_operation(
        &paths.installation_state_root(),
        &operation.plan_digest,
        &operation.completed_operation,
    );
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("restore-history.a3s-use-payload");
    let snapshot = session
        .snapshot_restore_coordinator(archive.clone(), started_at_ms + 10)
        .await
        .unwrap();
    SnapshotFixture {
        _temporary: temporary,
        registry,
        session,
        snapshot,
        archive,
    }
}

pub(in crate::control_store) fn registry() -> ControlPayloadOwnerRegistry {
    registry_with_limits(128, 128 * 1024 * 1024)
}

pub(in crate::control_store) fn registry_with_limits(
    max_files: u64,
    max_payload_bytes: u64,
) -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(|owner| {
                if owner == ControlPayloadOwnerId::ArtifactStore {
                    ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
                } else {
                    let schema = match owner {
                        ControlPayloadOwnerId::RestoreCoordinator => {
                            CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA.to_owned()
                        }
                        _ => format!("a3s.use.test.{}-snapshot.v1", owner.as_str()),
                    };
                    ControlPayloadOwnerRegistration::snapshotted(
                        owner,
                        schema,
                        ControlPayloadOwnerLimits::new(max_files, max_payload_bytes, 512 * 1024)
                            .unwrap(),
                    )
                    .unwrap()
                }
            })
            .collect(),
    )
    .unwrap()
}

pub(super) fn dummy_receipts(
    session: &ControlPayloadSnapshotSession,
    omitted: ControlPayloadOwnerId,
) -> Vec<ControlPayloadSnapshotReceipt> {
    ControlPayloadOwnerId::SNAPSHOTTED
        .into_iter()
        .filter(|owner| *owner != omitted)
        .map(|owner| {
            session
                .receipt(
                    owner,
                    ControlPayloadSnapshotEvidence::new(
                        digest(owner as u8 + 1),
                        digest(owner as u8 + 16),
                        1,
                        0,
                        0,
                    ),
                )
                .unwrap()
        })
        .collect()
}

fn digest(byte: u8) -> String {
    format!("sha256:{}", format!("{byte:02x}").repeat(32))
}
