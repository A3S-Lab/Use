use a3s_use_extension::ExtensionPaths;
use tempfile::TempDir;

use super::super::aggregate_tests::fixtures::control_installation;
use super::super::payload_host_projection_tests::support::{paths, registry, seed_host_projection};
use super::super::payload_owner::*;
use super::super::ControlStore;
use crate::cognitive_package::{
    host_projection_snapshot_fixture_sources, scan_host_projection_snapshot,
};

pub(in crate::control_store) struct VerifiedHostFixture {
    pub(in crate::control_store) registry: ControlPayloadOwnerRegistry,
    pub(in crate::control_store) snapshot: ControlHostProjectionSnapshot,
    pub(in crate::control_store) verified: VerifiedControlHostProjectionSnapshot,
    pub(in crate::control_store) sources: Vec<(String, Vec<u8>)>,
    _source: TempDir,
}

pub(in crate::control_store) async fn verified_host_fixture() -> VerifiedHostFixture {
    let source = TempDir::new().unwrap();
    let source_paths = paths(&source);
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let mut sources = seed_host_projection(&store, &source_paths).await;
    sources.sort_by(|left, right| left.0.cmp(&right.0));
    verified_fixture(source, store, sources).await
}

pub(in crate::control_store) async fn verified_absent_host_fixture() -> VerifiedHostFixture {
    let source = TempDir::new().unwrap();
    let source_paths = paths(&source);
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    verified_fixture(source, store, Vec::new()).await
}

async fn verified_fixture(
    source: TempDir,
    store: ControlStore,
    sources: Vec<(String, Vec<u8>)>,
) -> VerifiedHostFixture {
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = source.path().join("host-projection.archive");
    let snapshot = session
        .snapshot_host_projection(archive.clone(), 41_000)
        .await
        .unwrap();
    let archive = archive.exists().then_some(archive);
    let verified = snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            archive,
        )
        .await
        .unwrap();
    VerifiedHostFixture {
        registry,
        snapshot,
        verified,
        sources,
        _source: source,
    }
}

pub(in crate::control_store) fn target_paths(target: &TempDir) -> ExtensionPaths {
    paths(target)
}

pub(in crate::control_store) fn restore_staging(paths: &ExtensionPaths) -> std::path::PathBuf {
    paths
        .installation_state_root()
        .join("restore-staging/host-projection")
}

pub(in crate::control_store) fn live_host_root(paths: &ExtensionPaths) -> std::path::PathBuf {
    paths.installation_state_root().join("plugin-host-manager")
}

pub(in crate::control_store) async fn restored_sources(
    paths: &ExtensionPaths,
) -> Vec<(String, Vec<u8>)> {
    host_projection_snapshot_fixture_sources(
        &paths.installation_state_root(),
        &control_installation(),
    )
    .await
    .unwrap()
}

pub(in crate::control_store) async fn restored_index_count(paths: &ExtensionPaths) -> u64 {
    scan_host_projection_snapshot(
        &paths.installation_state_root(),
        &control_installation(),
        128,
        32 * 1024 * 1024,
    )
    .await
    .unwrap()
    .validated_index_records
}

pub(in crate::control_store) fn assert_canonical_indexes(
    paths: &ExtensionPaths,
    operations: usize,
    cancellations: usize,
    diagnostics: usize,
) {
    let root = live_host_root(paths);
    let scope_roots = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_dir() && path.file_name().unwrap() != "diagnostics")
        .collect::<Vec<_>>();
    assert_eq!(count_files(&scope_roots, "operations"), operations);
    assert_eq!(count_files(&scope_roots, "cancellations"), cancellations);
    assert_eq!(
        walk_files(&root.join("diagnostics/enablement"))
            .into_iter()
            .filter(|path| path.extension().is_some_and(|value| value == "json"))
            .count(),
        diagnostics
    );
    assert!(walk_files(&root)
        .into_iter()
        .all(|path| path.file_name().is_none_or(|name| name != ".store.lock")));
}

pub(in crate::control_store) fn write_existing_request(
    paths: &ExtensionPaths,
    source: &(String, Vec<u8>),
) {
    let target = live_host_root(paths).join(source.0.replace('/', std::path::MAIN_SEPARATOR_STR));
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(target, &source.1).unwrap();
}

fn count_files(scope_roots: &[std::path::PathBuf], family: &str) -> usize {
    scope_roots
        .iter()
        .flat_map(|root| walk_files(&root.join(family)))
        .count()
}

pub(in crate::control_store) fn walk_files(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    if !root.exists() {
        return Vec::new();
    }
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}
