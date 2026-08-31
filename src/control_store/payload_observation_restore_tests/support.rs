use std::path::Path;

use a3s_use_core::InstallationId;
use a3s_use_extension::ExtensionPaths;
use tempfile::TempDir;

use super::super::payload_knowledge_tests::support::{control_installation, paths, registry};
use super::super::payload_owner::{
    ControlObservationPayloadSnapshot, ControlPayloadOwnerRegistry,
    VerifiedControlObservationPayloadSnapshot,
};
use super::super::ControlStore;
use crate::cognitive_package::planning_observation_snapshot_fixtures;

pub(in crate::control_store) struct ObservationRestoreFixture {
    pub(in crate::control_store) _source: TempDir,
    pub(in crate::control_store) installation: InstallationId,
    pub(in crate::control_store) registry: ControlPayloadOwnerRegistry,
    pub(in crate::control_store) snapshot: ControlObservationPayloadSnapshot,
    pub(in crate::control_store) verified: VerifiedControlObservationPayloadSnapshot,
    records: Vec<(String, Vec<u8>)>,
}

impl ObservationRestoreFixture {
    pub(in crate::control_store) fn terminal_records(
        &self,
    ) -> impl Iterator<Item = &(String, Vec<u8>)> {
        self.records.iter().filter(|(path, _)| {
            path.starts_with("package-diagnostic-history/scopes/")
                || path == "package-resolutions/install/acme/knowledge.json"
        })
    }

    pub(in crate::control_store) fn active_record(&self) -> &(String, Vec<u8>) {
        self.records
            .iter()
            .find(|(path, _)| path == "package-resolutions/install/acme/pending.json")
            .unwrap()
    }
}

pub(in crate::control_store) async fn verified_observation_fixture() -> ObservationRestoreFixture {
    observation_fixture(false).await
}

pub(in crate::control_store) async fn verified_absent_observation_fixture(
) -> ObservationRestoreFixture {
    observation_fixture(true).await
}

async fn observation_fixture(active_only: bool) -> ObservationRestoreFixture {
    let source = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&source, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let records = planning_observation_snapshot_fixtures(&installation);
    if active_only {
        write_fixture(source_paths.state_root(), &records[1]);
    } else {
        for record in &records {
            write_fixture(source_paths.state_root(), record);
        }
    }
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = source.path().join("observations.a3s-use-payload");
    let snapshot = session
        .snapshot_planning_and_diagnostics(archive.clone(), 20_000)
        .await
        .unwrap();
    let binding = session.binding().clone();
    let control_export = session.control_export().to_vec();
    drop(session);
    let verified = snapshot
        .verify_offline(
            &registry,
            &binding,
            &control_export,
            (!active_only).then_some(archive),
        )
        .await
        .unwrap();
    ObservationRestoreFixture {
        _source: source,
        installation,
        registry,
        snapshot,
        verified,
        records,
    }
}

pub(in crate::control_store) fn restore_staging(paths: &ExtensionPaths) -> std::path::PathBuf {
    paths
        .installation_state_root()
        .join("operations/state-restores/control-fixture/observations")
}

pub(in crate::control_store) fn write_fixture(state_root: &Path, fixture: &(String, Vec<u8>)) {
    let path = state_root.join("operations").join(&fixture.0);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, &fixture.1).unwrap();
}
