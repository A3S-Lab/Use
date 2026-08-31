use a3s_use_core::{InstallationId, InstallationKind};
use tempfile::TempDir;

use super::payload_knowledge_tests::support::{control_installation, paths};
use super::payload_restore_coordinator_tests::support::snapshot_fixture;
use super::payload_restore_coordinator_tests::support::{registry, registry_with_limits};
use super::ControlStore;
use crate::state_restore::test_support::{
    restore_history_fixture, write_restore_history_operation,
};
use crate::state_restore::STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES;

#[tokio::test]
async fn snapshot_rejects_moved_foreign_nonterminal_and_unowned_history() {
    for case in [
        "moved",
        "foreign",
        "orphan",
        "corrupt",
        "temporary",
        "unknown",
        "unrecorded",
        "pruning",
        "marker-temporary",
        "marker-corrupt",
    ] {
        let temporary = TempDir::new().unwrap();
        let installation = control_installation();
        let paths = paths(&temporary, installation.clone());
        let store = ControlStore::from_extension_paths(&paths).unwrap();
        store.initialize().await.unwrap();
        let fixture_installation = if case == "foreign" {
            InstallationId::new(InstallationKind::User, "foreign/installation").unwrap()
        } else {
            installation
        };
        let operation = restore_history_fixture(&fixture_installation, 7_000).await;
        let digest = if case == "moved" {
            format!("sha256:{}", "a".repeat(64))
        } else {
            operation.plan_digest.clone()
        };
        let bytes = match case {
            "orphan" => operation.planned_operation,
            "corrupt" => b"{}".to_vec(),
            _ => operation.completed_operation,
        };
        if case != "unrecorded" {
            write_restore_history_operation(&paths.installation_state_root(), &digest, &bytes);
        }
        let directory = paths
            .installation_state_root()
            .join("operations/state-restores")
            .join(digest.strip_prefix("sha256:").unwrap());
        match case {
            "temporary" => {
                std::fs::write(directory.join("operation.json.tmp"), b"partial").unwrap()
            }
            "unknown" => std::fs::write(directory.join("unknown.json"), b"{}").unwrap(),
            "unrecorded" => std::fs::create_dir_all(&directory).unwrap(),
            "pruning" => std::fs::rename(
                &directory,
                directory.with_file_name(format!(
                    ".pruning-{}",
                    digest.strip_prefix("sha256:").unwrap()
                )),
            )
            .unwrap(),
            "marker-temporary" => std::fs::write(
                paths
                    .installation_state_root()
                    .join(".maintenance.restore.json.tmp"),
                b"partial",
            )
            .unwrap(),
            "marker-corrupt" => std::fs::write(
                paths
                    .installation_state_root()
                    .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER),
                b"{}",
            )
            .unwrap(),
            _ => {}
        }

        let session = store.begin_payload_snapshot(registry()).await.unwrap();
        assert_eq!(
            session
                .snapshot_restore_coordinator(
                    temporary.path().join(format!("{case}.archive")),
                    8_000,
                )
                .await
                .unwrap_err()
                .code,
            "use.control_store.restore_coordinator_snapshot_invalid",
            "case {case} was accepted"
        );
    }
}

#[tokio::test]
async fn snapshot_rejects_destinations_inside_state_or_already_owned() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&target_paths).unwrap();
    store.initialize().await.unwrap();
    let operation = restore_history_fixture(&installation, 9_000).await;
    write_restore_history_operation(
        &target_paths.installation_state_root(),
        &operation.plan_digest,
        &operation.completed_operation,
    );
    let session = store.begin_payload_snapshot(registry()).await.unwrap();

    let inside = target_paths
        .installation_state_root()
        .join("inside.archive");
    assert_eq!(
        session
            .snapshot_restore_coordinator(inside, 9_100)
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
    let existing = temporary.path().join("existing.archive");
    std::fs::write(&existing, b"owned").unwrap();
    assert_eq!(
        session
            .snapshot_restore_coordinator(existing.clone(), 9_200)
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
    assert_eq!(std::fs::read(existing).unwrap(), b"owned");
}

#[tokio::test]
async fn registered_limits_cover_excluded_active_files_and_terminal_bytes() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let target_paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&target_paths).unwrap();
    store.initialize().await.unwrap();
    let operation = restore_history_fixture(&installation, 9_500).await;
    write_restore_history_operation(
        &target_paths.installation_state_root(),
        &operation.plan_digest,
        &operation.planned_operation,
    );
    std::fs::write(
        target_paths
            .installation_state_root()
            .join(a3s_use_extension::ACTIVE_STATE_RESTORE_MARKER),
        &operation.active_marker,
    )
    .unwrap();
    let session = store
        .begin_payload_snapshot(registry_with_limits(1, 128 * 1024 * 1024))
        .await
        .unwrap();
    assert_eq!(
        session
            .snapshot_restore_coordinator(temporary.path().join("active-limit.archive"), 9_510)
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );

    let terminal_root = TempDir::new().unwrap();
    let terminal_paths = paths(&terminal_root, installation);
    let terminal_store = ControlStore::from_extension_paths(&terminal_paths).unwrap();
    terminal_store.initialize().await.unwrap();
    write_restore_history_operation(
        &terminal_paths.installation_state_root(),
        &operation.plan_digest,
        &operation.completed_operation,
    );
    let session = terminal_store
        .begin_payload_snapshot(registry_with_limits(
            128,
            operation.completed_operation.len() as u64 - 1,
        ))
        .await
        .unwrap();
    assert_eq!(
        session
            .snapshot_restore_coordinator(
                terminal_root.path().join("payload-limit.archive"),
                9_520,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
}

#[tokio::test]
async fn offline_manifest_cannot_exceed_native_restore_history_capacity() {
    let fixture = snapshot_fixture(9_600).await;
    let mut snapshot = fixture.snapshot.clone();
    let template = snapshot.manifest.entries[0].clone();
    snapshot.manifest.entries = (0..=STATE_RESTORE_HISTORY_SNAPSHOT_MAX_OPERATION_FILES)
        .map(|index| {
            let mut entry = template.clone();
            entry.plan_digest = format!("sha256:{index:064x}");
            entry.length = 1;
            entry
        })
        .collect();

    let error = snapshot
        .verify_offline(
            &fixture.registry,
            fixture.session.binding(),
            fixture.session.control_export(),
            Some(fixture.archive),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
    assert_eq!(
        error.message,
        "The Restore Coordinator payload exceeds its native or registered bounds."
    );
}

#[cfg(unix)]
#[tokio::test]
async fn offline_verification_rejects_a_linked_archive() {
    let fixture = snapshot_fixture(9_700).await;
    let link = fixture
        .archive
        .with_file_name("linked-restore-history.archive");
    std::os::unix::fs::symlink(&fixture.archive, &link).unwrap();
    assert_eq!(
        fixture
            .snapshot
            .verify_offline(
                &fixture.registry,
                fixture.session.binding(),
                fixture.session.control_export(),
                Some(link),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn snapshot_rejects_linked_history() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let operation = restore_history_fixture(&installation, 10_000).await;
    let outside = temporary.path().join("outside");
    std::fs::create_dir(&outside).unwrap();
    write_restore_history_operation(
        &outside,
        &operation.plan_digest,
        &operation.completed_operation,
    );
    std::fs::create_dir_all(paths.installation_state_root().join("operations")).unwrap();
    crate::test_filesystem::create_directory_link(
        &outside.join("operations/state-restores"),
        &paths
            .installation_state_root()
            .join("operations/state-restores"),
    );
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    assert_eq!(
        session
            .snapshot_restore_coordinator(temporary.path().join("linked.archive"), 10_100)
            .await
            .unwrap_err()
            .code,
        "use.control_store.restore_coordinator_snapshot_invalid"
    );
}
