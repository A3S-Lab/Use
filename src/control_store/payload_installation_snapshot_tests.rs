use a3s_use_extension::ExtensionPaths;
use tempfile::TempDir;

use super::aggregate_tests::fixtures::{control_installation, operation};
use super::payload_host_projection_tests::support::seed_host_projection_for_completed_operation;
use super::payload_knowledge_tests::support::seed_control_knowledge;
use super::payload_owner::*;
use super::ControlStore;
use crate::cognitive_package::planning_observation_snapshot_fixtures;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation,
};

#[tokio::test]
async fn complete_snapshot_archive_round_trips_one_bound_owner_set() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let completed = operation("knowledge-snapshot-operation");
    seed_control_knowledge(&store, &paths).await;
    let host_entries =
        seed_host_projection_for_completed_operation(&store, &paths, &completed).await;
    seed_observations(&paths, &installation);
    let restore = restore_history_fixture(&installation, 2_000).await;
    write_restore_history_operation(
        &paths.installation_state_root(),
        &restore.plan_digest,
        &restore.completed_operation,
    );

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let destination = temporary.path().join("complete.a3s-use-control-snapshot");
    let manifest = session
        .snapshot_complete_set(
            destination.clone(),
            OkfKnowledgeStoragePolicy::default(),
            3_000,
        )
        .await
        .unwrap();

    manifest.validate(&registry).unwrap();
    assert_eq!(manifest.snapshot_set.binding, *session.binding());
    assert_eq!(manifest.snapshot_set.receipts.len(), 5);
    assert_eq!(
        manifest
            .snapshot_set
            .receipts
            .iter()
            .map(|receipt| receipt.owner)
            .collect::<Vec<_>>(),
        ControlPayloadOwnerId::SNAPSHOTTED
    );
    assert!(manifest.snapshot_set.file_count > 0);
    assert!(matches!(
        &manifest.host_projection.manifest.payload,
        ControlHostProjectionState::Archive { .. }
    ));
    assert_eq!(
        manifest.host_projection.receipt.file_count,
        host_entries.len() as u64
    );
    assert!(matches!(
        &manifest.knowledge.manifest.payload,
        ControlKnowledgePayloadState::Archive { .. }
    ));
    assert!(destination.is_file());

    let encoded = serde_json::to_string(&manifest).unwrap();
    assert!(!encoded.contains(&temporary.path().display().to_string()));
    assert!(!encoded.contains("archivePath"));
    assert!(!encoded.contains("stateRoot"));

    let verified =
        VerifiedControlInstallationSnapshot::verify_offline(registry.clone(), destination)
            .await
            .unwrap();
    assert_eq!(verified.manifest(), &manifest);
    assert_eq!(verified.control_export(), session.control_export());
    assert_eq!(verified.manifest().observations.manifest.entries.len(), 2);
    assert_eq!(
        verified
            .manifest()
            .restore_coordinator
            .manifest
            .entries
            .len(),
        1
    );
}

#[tokio::test]
async fn complete_snapshot_archive_represents_five_absent_payloads_without_files() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let destination = temporary.path().join("empty.a3s-use-control-snapshot");

    let manifest = session
        .snapshot_complete_set(
            destination.clone(),
            OkfKnowledgeStoragePolicy::default(),
            4_000,
        )
        .await
        .unwrap();
    assert_eq!(manifest.snapshot_set.file_count, 0);
    assert_eq!(manifest.snapshot_set.byte_count, 0);
    assert!(matches!(
        manifest.host_projection.manifest.payload,
        ControlHostProjectionState::Absent
    ));
    assert!(matches!(
        manifest.knowledge.manifest.payload,
        ControlKnowledgePayloadState::Absent
    ));
    assert!(matches!(
        manifest.observations.manifest.payload,
        ControlObservationPayloadState::Absent
    ));
    assert!(matches!(
        manifest.restore_coordinator.manifest.payload,
        ControlRestoreCoordinatorState::Absent
    ));
    assert!(matches!(
        manifest.runtime_plans.manifest.payload,
        ControlRuntimePlanPayloadState::Absent
    ));

    let verified = VerifiedControlInstallationSnapshot::verify_offline(registry, destination)
        .await
        .unwrap();
    assert_eq!(verified.manifest(), &manifest);
}

#[tokio::test]
async fn complete_snapshot_manifest_rejects_cross_owner_rebinding() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let manifest = session
        .snapshot_complete_set(
            temporary.path().join("binding.snapshot"),
            OkfKnowledgeStoragePolicy::default(),
            4_100,
        )
        .await
        .unwrap();

    let mut timestamp = manifest.clone();
    timestamp.knowledge.manifest.created_at_ms += 1;
    assert_eq!(
        timestamp.validate(&registry).unwrap_err().code,
        "use.control_store.complete_snapshot_invalid"
    );

    let mut receipts = manifest;
    receipts.snapshot_set.receipts.swap(0, 1);
    assert_eq!(
        receipts.validate(&registry).unwrap_err().code,
        "use.control_store.complete_snapshot_invalid"
    );
}

#[test]
fn complete_snapshot_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlInstallationSnapshotManifest>();
    assert_send_sync::<VerifiedControlInstallationSnapshot>();
}

pub(in crate::control_store) fn paths(temporary: &TempDir) -> ExtensionPaths {
    ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        control_installation(),
    )
    .unwrap()
}

pub(in crate::control_store) fn registry() -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(|owner| {
                if owner == ControlPayloadOwnerId::ArtifactStore {
                    ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
                } else {
                    let schema = match owner {
                        ControlPayloadOwnerId::HostProtocolProjection => {
                            CONTROL_HOST_PROJECTION_SNAPSHOT_SCHEMA
                        }
                        ControlPayloadOwnerId::KnowledgePayload => {
                            CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA
                        }
                        ControlPayloadOwnerId::PlanningAndDiagnosticObservations => {
                            CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA
                        }
                        ControlPayloadOwnerId::RestoreCoordinator => {
                            CONTROL_RESTORE_COORDINATOR_SNAPSHOT_SCHEMA
                        }
                        ControlPayloadOwnerId::RuntimePlanPayload => {
                            CONTROL_RUNTIME_PLAN_PAYLOAD_SNAPSHOT_SCHEMA
                        }
                        ControlPayloadOwnerId::ArtifactStore => unreachable!(),
                    };
                    ControlPayloadOwnerRegistration::snapshotted(
                        owner,
                        schema,
                        ControlPayloadOwnerLimits::new(128, 128 * 1024 * 1024, 512 * 1024).unwrap(),
                    )
                    .unwrap()
                }
            })
            .collect(),
    )
    .unwrap()
}

pub(in crate::control_store) fn seed_observations(
    paths: &ExtensionPaths,
    installation: &a3s_use_core::InstallationId,
) {
    for (relative, bytes) in planning_observation_snapshot_fixtures(installation) {
        let path = paths
            .installation_state_root()
            .join("operations")
            .join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
}
