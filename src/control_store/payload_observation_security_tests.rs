use a3s_use_core::InstallationId;
use tempfile::TempDir;

use super::payload_knowledge_tests::support::{control_installation, paths};
use super::payload_owner::*;
use super::ControlStore;
use crate::cognitive_package::planning_observation_snapshot_fixtures;

#[tokio::test]
async fn operational_locks_are_excluded_but_unknown_empty_layout_is_rejected() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    write_fixtures(paths.state_root(), &installation);
    let scope = installation.storage_key().unwrap();
    for relative in [
        format!("package-diagnostic-history/locks/{scope}/acme/knowledge.lock"),
        "package-downloads/locks/acme/knowledge.lock".to_owned(),
    ] {
        let path = paths.state_root().join("operations").join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"").unwrap();
    }

    let session = store
        .begin_payload_snapshot(registry_with_limits(16, 16 * 1024 * 1024))
        .await
        .unwrap();
    let snapshot = session
        .snapshot_planning_and_diagnostics(temporary.path().join("locks.archive"), 7_000)
        .await
        .unwrap();
    assert_eq!(snapshot.receipt.file_count, 2);
    assert_eq!(snapshot.manifest.excluded_active_records, 1);
    drop(session);

    let unknown = paths
        .state_root()
        .join("operations/package-resolutions/unknown");
    std::fs::create_dir_all(unknown).unwrap();
    let session = store
        .begin_payload_snapshot(registry_with_limits(16, 16 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(
        session
            .snapshot_planning_and_diagnostics(temporary.path().join("unknown.archive"), 7_001,)
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
}

#[tokio::test]
async fn archive_length_and_manifest_tampering_fail_closed() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    write_fixtures(paths.state_root(), &installation);
    let registry = registry_with_limits(16, 16 * 1024 * 1024);
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("security.archive");
    let snapshot = session
        .snapshot_planning_and_diagnostics(archive.clone(), 8_000)
        .await
        .unwrap();

    let original = std::fs::read(&archive).unwrap();
    let mut trailing = original.clone();
    trailing.push(0);
    std::fs::write(&archive, trailing).unwrap();
    assert_eq!(
        snapshot
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
    std::fs::write(&archive, &original[..original.len() - 1]).unwrap();
    assert_eq!(
        snapshot
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
    std::fs::write(&archive, original).unwrap();

    let mut rebound = snapshot.clone();
    rebound.manifest.excluded_active_records += 1;
    assert_eq!(
        rebound
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
}

#[tokio::test]
async fn registered_file_and_byte_limits_cover_excluded_active_records() {
    for (max_files, max_bytes) in [(1, 16 * 1024 * 1024), (16, 32)] {
        let temporary = TempDir::new().unwrap();
        let installation = control_installation();
        let paths = paths(&temporary, installation.clone());
        let store = ControlStore::from_extension_paths(&paths).unwrap();
        store.initialize().await.unwrap();
        write_fixtures(paths.state_root(), &installation);
        let session = store
            .begin_payload_snapshot(registry_with_limits(max_files, max_bytes))
            .await
            .unwrap();
        assert_eq!(
            session
                .snapshot_planning_and_diagnostics(
                    temporary
                        .path()
                        .join(format!("limit-{max_files}-{max_bytes}.archive")),
                    9_000,
                )
                .await
                .unwrap_err()
                .code,
            "use.control_store.observation_payload_snapshot_invalid"
        );
    }
}

#[tokio::test]
async fn duplicate_resolution_identity_across_action_directories_is_rejected() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let terminal = planning_observation_snapshot_fixtures(&installation).remove(0);
    write_fixture(paths.state_root(), &terminal);
    let mut value: serde_json::Value = serde_json::from_slice(&terminal.1).unwrap();
    value["action"] = serde_json::json!("upgrade");
    write_fixture(
        paths.state_root(),
        &(
            "package-resolutions/upgrade/acme/knowledge.json".to_owned(),
            serde_json::to_vec(&value).unwrap(),
        ),
    );

    let session = store
        .begin_payload_snapshot(registry_with_limits(16, 16 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(
        session
            .snapshot_planning_and_diagnostics(temporary.path().join("duplicate.archive"), 10_000,)
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
}

fn write_fixtures(state_root: &std::path::Path, installation: &InstallationId) {
    for fixture in planning_observation_snapshot_fixtures(installation) {
        write_fixture(state_root, &fixture);
    }
}

fn write_fixture(state_root: &std::path::Path, fixture: &(String, Vec<u8>)) {
    let path = state_root.join("operations").join(&fixture.0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &fixture.1).unwrap();
}

fn registry_with_limits(max_files: u64, max_payload_bytes: u64) -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(|owner| {
                if owner == ControlPayloadOwnerId::ArtifactStore {
                    ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
                } else {
                    let schema = match owner {
                        ControlPayloadOwnerId::KnowledgePayload => {
                            CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA.to_owned()
                        }
                        ControlPayloadOwnerId::PlanningAndDiagnosticObservations => {
                            CONTROL_OBSERVATION_PAYLOAD_SNAPSHOT_SCHEMA.to_owned()
                        }
                        _ => format!("a3s.use.test.{}-snapshot.v1", owner.as_str()),
                    };
                    ControlPayloadOwnerRegistration::snapshotted(
                        owner,
                        schema,
                        ControlPayloadOwnerLimits::new(max_files, max_payload_bytes, 256 * 1024)
                            .unwrap(),
                    )
                    .unwrap()
                }
            })
            .collect(),
    )
    .unwrap()
}
