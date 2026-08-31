pub(in crate::control_store) mod support;

use tempfile::TempDir;

use super::payload_owner::*;
use super::ControlStore;
#[cfg(unix)]
use support::first_host_request_path;
use support::{
    paths, registry, remove_one_cancellation_alias, remove_operation_indexes,
    seed_host_desired_state_drift, seed_host_no_change, seed_host_projection,
};

#[tokio::test]
async fn host_projection_sources_are_control_bound_and_indexes_are_not_archived() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let mut expected = seed_host_projection(&store, &paths).await;
    expected.sort_by(|left, right| left.0.cmp(&right.0));

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("host-projection.a3s-use-payload");
    let snapshot = session
        .snapshot_host_projection(archive.clone(), 30_000)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.binding, *session.binding());
    assert_eq!(snapshot.manifest.entries.len(), 3);
    assert_eq!(
        snapshot
            .manifest
            .entries
            .iter()
            .map(|entry| entry.kind)
            .collect::<Vec<_>>(),
        [
            ControlHostProjectionEntryKind::Cancellation,
            ControlHostProjectionEntryKind::Request,
            ControlHostProjectionEntryKind::Request,
        ]
    );
    assert_eq!(
        snapshot
            .manifest
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>()
    );
    let expected_archive = expected
        .iter()
        .flat_map(|(_, bytes)| bytes.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(std::fs::read(&archive).unwrap(), expected_archive);
    assert_eq!(snapshot.receipt.file_count, 3);
    assert_eq!(
        snapshot.receipt.byte_count,
        expected
            .iter()
            .map(|(_, bytes)| bytes.len() as u64)
            .sum::<u64>()
    );
    assert!(snapshot.manifest.validated_index_records >= 3);

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
}

#[tokio::test]
async fn empty_host_projection_is_an_explicit_zero_file_snapshot() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("empty-host-projection.archive");

    let snapshot = session
        .snapshot_host_projection(archive.clone(), 31_000)
        .await
        .unwrap();

    assert_eq!(
        snapshot.manifest.payload,
        ControlHostProjectionState::Absent
    );
    assert!(snapshot.manifest.entries.is_empty());
    assert_eq!(snapshot.manifest.validated_index_records, 0);
    assert_eq!(snapshot.receipt.file_count, 0);
    assert_eq!(snapshot.receipt.byte_count, 0);
    assert!(!archive.exists());
    snapshot
        .verify_offline(&registry, session.binding(), session.control_export(), None)
        .await
        .unwrap();
}

#[tokio::test]
async fn host_projection_offline_verification_rejects_archive_and_control_substitution() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_projection(&store, &paths).await;
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("host-tamper.archive");
    let snapshot = session
        .snapshot_host_projection(archive.clone(), 32_000)
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
        "use.control_store.host_projection_snapshot_invalid"
    );
}

#[test]
fn host_projection_snapshot_contracts_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ControlHostProjectionSnapshot>();
    assert_send_sync::<VerifiedControlHostProjectionSnapshot>();
}

#[tokio::test]
async fn no_change_host_request_is_preserved_without_inventing_an_operation_index() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_no_change(&store, &paths).await;
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("host-no-change.archive");

    let snapshot = session
        .snapshot_host_projection(archive.clone(), 33_000)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.entries.len(), 1);
    assert_eq!(
        snapshot.manifest.entries[0].kind,
        ControlHostProjectionEntryKind::Request
    );
    assert_eq!(snapshot.manifest.validated_index_records, 0);
    snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn host_snapshot_rejects_missing_derived_operation_indexes() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_projection(&store, &paths).await;
    remove_operation_indexes(&paths);
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("host-missing-index.archive");

    let error = session
        .snapshot_host_projection(archive.clone(), 34_000)
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.control_store.host_projection_snapshot_invalid"
    );
    assert!(!archive.exists());
}

#[tokio::test]
async fn one_legacy_or_exact_cancellation_alias_normalizes_to_one_semantic_record() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_projection(&store, &paths).await;
    remove_one_cancellation_alias(&paths);
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("host-legacy-cancellation.archive");

    let snapshot = session
        .snapshot_host_projection(archive.clone(), 35_000)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.entries.len(), 3);
    assert_eq!(
        snapshot
            .manifest
            .entries
            .iter()
            .filter(|entry| entry.kind == ControlHostProjectionEntryKind::Cancellation)
            .count(),
        1
    );
    snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn control_desired_state_drift_is_rejected_before_archive_publication() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_desired_state_drift(&store, &paths).await;
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("host-control-drift.archive");

    let error = session
        .snapshot_host_projection(archive.clone(), 36_000)
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.control_store.host_projection_snapshot_invalid"
    );
    assert!(!archive.exists());
}

#[tokio::test]
async fn host_snapshot_never_clobbers_a_caller_owned_destination() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_projection(&store, &paths).await;
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("existing-host.archive");
    std::fs::write(&archive, b"caller-owned").unwrap();

    let error = session
        .snapshot_host_projection(archive.clone(), 37_000)
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.control_store.host_projection_snapshot_invalid"
    );
    assert_eq!(std::fs::read(archive).unwrap(), b"caller-owned");
}

#[tokio::test]
async fn host_snapshot_rejects_a_destination_inside_use_owned_state() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_projection(&store, &paths).await;
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = paths.state_root().join("forbidden-host.archive");

    let error = session
        .snapshot_host_projection(archive.clone(), 38_000)
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.control_store.host_projection_snapshot_invalid"
    );
    assert!(!archive.exists());
}

#[cfg(unix)]
#[tokio::test]
async fn host_snapshot_rejects_linked_semantic_records() {
    use std::os::unix::fs::symlink;

    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_host_projection(&store, &paths).await;
    let request = first_host_request_path(&paths);
    let outside = temporary.path().join("outside-host-request.json");
    std::fs::copy(&request, &outside).unwrap();
    std::fs::remove_file(&request).unwrap();
    symlink(outside, request).unwrap();
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("linked-host.archive");

    let error = session
        .snapshot_host_projection(archive.clone(), 39_000)
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.control_store.host_projection_snapshot_invalid"
    );
    assert!(!archive.exists());
}
