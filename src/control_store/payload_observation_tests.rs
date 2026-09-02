use std::path::Path;

use a3s_use_core::{InstallationId, InstallationKind};
use tempfile::TempDir;

use super::payload_knowledge_tests::support::{control_installation, paths, registry};
use super::payload_owner::*;
use super::ControlStore;
use crate::cognitive_package::planning_observation_snapshot_fixtures;

#[tokio::test]
async fn terminal_observations_are_control_bound_and_active_attempts_are_excluded() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let fixtures = seed_observations(paths.state_root(), &installation);

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("observations.a3s-use-payload");
    let snapshot = session
        .snapshot_planning_and_diagnostics(archive.clone(), 1_000)
        .await
        .unwrap();

    let diagnostic = fixtures
        .iter()
        .find(|(path, _)| path.starts_with("package-diagnostic-history/"))
        .unwrap();
    let terminal = fixtures
        .iter()
        .find(|(path, _)| path == "package-resolutions/install/acme/knowledge.json")
        .unwrap();
    let expected_archive = [diagnostic.1.as_slice(), terminal.1.as_slice()].concat();

    assert_eq!(snapshot.manifest.binding, *session.binding());
    assert_eq!(snapshot.manifest.entries.len(), 2);
    assert_eq!(snapshot.manifest.excluded_active_records, 1);
    assert_eq!(
        snapshot.manifest.entries[0].kind,
        ControlObservationPayloadEntryKind::DiagnosticHistory
    );
    assert_eq!(snapshot.manifest.entries[0].path, diagnostic.0);
    assert_eq!(
        snapshot.manifest.entries[0].length,
        diagnostic.1.len() as u64
    );
    assert_eq!(
        snapshot.manifest.entries[1].kind,
        ControlObservationPayloadEntryKind::TerminalResolution
    );
    assert_eq!(
        snapshot.manifest.entries[1].path,
        "package-resolutions/install/acme/knowledge.json"
    );
    assert_eq!(snapshot.manifest.entries[1].length, terminal.1.len() as u64);
    assert_eq!(snapshot.receipt.file_count, 2);
    assert_eq!(snapshot.receipt.byte_count, expected_archive.len() as u64);
    assert!(archive.is_file());
    assert_eq!(std::fs::read(&archive).unwrap(), expected_archive);

    let encoded = serde_json::to_string(&snapshot).unwrap();
    assert!(!encoded.contains(&temporary.path().display().to_string()));
    snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();

    let mut receipts = ControlPayloadOwnerId::SNAPSHOTTED
        .into_iter()
        .filter(|owner| *owner != ControlPayloadOwnerId::PlanningAndDiagnosticObservations)
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
        .collect::<Vec<_>>();
    receipts.push(snapshot.receipt);
    let complete = session.complete(receipts).unwrap();
    assert_eq!(complete.receipts.len(), 5);
    assert_eq!(
        complete
            .receipts
            .iter()
            .map(|receipt| receipt.owner)
            .collect::<Vec<_>>(),
        ControlPayloadOwnerId::SNAPSHOTTED
    );
}

#[test]
fn observation_snapshot_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlObservationPayloadSnapshot>();
    assert_send_sync::<VerifiedControlObservationPayloadSnapshot>();
}

#[tokio::test]
async fn empty_or_active_only_inventory_creates_no_archive() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let fixtures = planning_observation_snapshot_fixtures(&installation);
    write_fixture(paths.state_root(), &fixtures[1]);

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("active-only.a3s-use-payload");
    let snapshot = session
        .snapshot_planning_and_diagnostics(archive.clone(), 2_000)
        .await
        .unwrap();
    assert_eq!(
        snapshot.manifest.payload,
        ControlObservationPayloadState::Absent
    );
    assert!(snapshot.manifest.entries.is_empty());
    assert_eq!(snapshot.manifest.excluded_active_records, 1);
    assert_eq!(snapshot.receipt.file_count, 0);
    assert_eq!(snapshot.receipt.byte_count, 0);
    assert!(!archive.exists());
    snapshot
        .verify_offline(&registry, session.binding(), session.control_export(), None)
        .await
        .unwrap();
    assert_eq!(
        snapshot
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(temporary.path().join("missing")),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
}

#[tokio::test]
async fn offline_verification_rejects_archive_and_control_substitution() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_observations(paths.state_root(), &installation);

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("tamper.a3s-use-payload");
    let snapshot = session
        .snapshot_planning_and_diagnostics(archive.clone(), 3_000)
        .await
        .unwrap();
    assert_eq!(
        snapshot
            .verify_offline(&registry, session.binding(), b"{}", Some(archive.clone()))
            .await
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut bytes = std::fs::read(&archive).unwrap();
    bytes[0] ^= 1;
    std::fs::write(&archive, bytes).unwrap();
    assert_eq!(
        snapshot
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
async fn snapshot_rejects_moved_corrupt_unknown_and_foreign_records() {
    for case in ["moved", "corrupt", "unknown", "foreign"] {
        let temporary = TempDir::new().unwrap();
        let installation = control_installation();
        let paths = paths(&temporary, installation.clone());
        let store = ControlStore::from_extension_paths(&paths).unwrap();
        store.initialize().await.unwrap();
        let mut fixtures = planning_observation_snapshot_fixtures(&installation);
        let fixture = match case {
            "moved" => {
                fixtures[0].0 = "package-resolutions/upgrade/acme/knowledge.json".to_owned();
                fixtures.remove(0)
            }
            "corrupt" => {
                fixtures[0].1 = b"{}".to_vec();
                fixtures.remove(0)
            }
            "unknown" => (
                "package-resolutions/other/acme/knowledge.json".to_owned(),
                fixtures.remove(0).1,
            ),
            "foreign" => planning_observation_snapshot_fixtures(
                &InstallationId::new(InstallationKind::User, "foreign/installation").unwrap(),
            )
            .remove(0),
            _ => unreachable!(),
        };
        write_fixture(paths.state_root(), &fixture);
        let session = store.begin_payload_snapshot(registry()).await.unwrap();
        assert_eq!(
            session
                .snapshot_planning_and_diagnostics(
                    temporary.path().join(format!("{case}.archive")),
                    4_000,
                )
                .await
                .unwrap_err()
                .code,
            "use.control_store.observation_payload_snapshot_invalid",
            "case {case} was accepted"
        );
    }
}

#[tokio::test]
async fn snapshot_rejects_in_state_destination_and_existing_destination() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_observations(paths.state_root(), &installation);
    let registry = registry();
    let session = store.begin_payload_snapshot(registry).await.unwrap();
    assert_eq!(
        session
            .snapshot_planning_and_diagnostics(
                paths.state_root().join("observations.archive"),
                5_000,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );

    let existing = temporary.path().join("existing.archive");
    std::fs::write(&existing, b"sentinel").unwrap();
    assert_eq!(
        session
            .snapshot_planning_and_diagnostics(existing.clone(), 5_001)
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
    assert_eq!(std::fs::read(existing).unwrap(), b"sentinel");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn snapshot_rejects_linked_owner_root_without_touching_the_target() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let outside = temporary.path().join("outside");
    std::fs::create_dir_all(&outside).unwrap();
    std::fs::write(outside.join("sentinel"), b"outside").unwrap();
    let operations = paths.state_root().join("operations");
    std::fs::create_dir_all(&operations).unwrap();
    let linked = operations.join("package-resolutions");
    crate::test_filesystem::create_directory_link(&outside, &linked);

    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    assert_eq!(
        session
            .snapshot_planning_and_diagnostics(temporary.path().join("linked.archive"), 6_000,)
            .await
            .unwrap_err()
            .code,
        "use.control_store.observation_payload_snapshot_invalid"
    );
    assert_eq!(std::fs::read(outside.join("sentinel")).unwrap(), b"outside");
    crate::test_filesystem::remove_directory_link(&linked);
}

fn seed_observations(state_root: &Path, installation: &InstallationId) -> Vec<(String, Vec<u8>)> {
    let fixtures = planning_observation_snapshot_fixtures(installation);
    for fixture in &fixtures {
        write_fixture(state_root, fixture);
    }
    fixtures
}

fn write_fixture(state_root: &Path, fixture: &(String, Vec<u8>)) {
    let path = state_root.join("operations").join(&fixture.0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &fixture.1).unwrap();
}

fn digest(seed: u8) -> String {
    format!("sha256:{}", format!("{seed:02x}").repeat(32))
}
