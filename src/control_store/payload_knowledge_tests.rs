use a3s_use_core::{InstallationId, InstallationKind, OkfKnowledgeObservedState};
use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_owner::*;
use super::ControlStore;
use crate::okf_knowledge::{OkfKnowledgeStoragePolicy, SqliteOkfKnowledgeAdapter};

mod support;

use support::*;

#[tokio::test]
async fn knowledge_snapshot_is_control_bound_path_free_and_offline_verified() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_control_knowledge(&store, &paths).await;

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("knowledge.a3s-okf-backup");
    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 1_000)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.binding, *session.binding());
    assert_eq!(snapshot.manifest.retained_bindings, 1);
    assert_eq!(snapshot.manifest.selected_surfaces, 1);
    assert_eq!(
        snapshot.receipt.owner,
        ControlPayloadOwnerId::KnowledgePayload
    );
    assert_eq!(snapshot.receipt.file_count, 1);
    assert_eq!(
        snapshot.receipt.byte_count,
        std::fs::metadata(&archive).unwrap().len()
    );
    assert_eq!(
        snapshot.receipt.owner_manifest_digest,
        snapshot.manifest.descriptor_digest
    );
    assert_eq!(
        snapshot.receipt.inventory_digest,
        snapshot.manifest.inventory_digest
    );

    let json = serde_json::to_string(&snapshot.manifest).unwrap();
    let temporary_path = temporary.path().to_string_lossy();
    assert!(!json.contains(temporary_path.as_ref() as &str));
    assert!(!json.contains("absolutePath"));
    assert!(!json.contains("stateRoot"));

    let verified = snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive.clone()),
        )
        .await
        .unwrap();
    assert_eq!(verified.bindings().len(), 1);
    assert_eq!(verified.selected().len(), 1);
    assert_eq!(
        snapshot
            .verify_offline(&registry, session.binding(), b"{}", Some(archive.clone()))
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_snapshot_invalid"
    );
    assert_eq!(
        snapshot
            .verify_offline(&registry, session.binding(), session.control_export(), None)
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_snapshot_invalid"
    );

    let decoded: ControlKnowledgePayloadSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    decoded
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive.clone()),
        )
        .await
        .unwrap();
    let mut rebound_json = serde_json::to_value(&snapshot).unwrap();
    rebound_json["manifest"]["binding"]["controlExportDigest"] =
        serde_json::Value::String(digest('f'));
    let rebound: ControlKnowledgePayloadSnapshot = serde_json::from_value(rebound_json).unwrap();
    assert_eq!(
        rebound
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.payload_snapshot_invalid"
    );

    let mut bytes = std::fs::read(&archive).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
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
        "use.control_store.knowledge_payload_snapshot_invalid"
    );
}

#[tokio::test]
async fn absent_knowledge_is_an_explicit_zero_file_snapshot_without_live_mutation() {
    let temporary = TempDir::new().unwrap();
    let installation = InstallationId::new(InstallationKind::User, "knowledge-absent").unwrap();
    let paths = paths(&temporary, installation);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("absent.a3s-okf-backup");

    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 2_000)
        .await
        .unwrap();

    assert!(matches!(
        &snapshot.manifest.payload,
        ControlKnowledgePayloadState::Absent
    ));
    assert_eq!(snapshot.manifest.retained_bindings, 0);
    assert_eq!(snapshot.manifest.selected_surfaces, 0);
    assert_eq!(snapshot.receipt.file_count, 0);
    assert_eq!(snapshot.receipt.byte_count, 0);
    assert!(!archive.exists());
    assert!(!paths.installation_state_root().join("knowledge").exists());
    let verified = snapshot
        .verify_offline(&registry, session.binding(), session.control_export(), None)
        .await
        .unwrap();
    assert!(verified.bindings().is_empty());
    assert!(verified.selected().is_empty());

    std::fs::write(&archive, b"do-not-overwrite").unwrap();
    assert_eq!(
        session
            .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 3_000,)
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_backup_exists"
    );
    assert_eq!(std::fs::read(&archive).unwrap(), b"do-not-overwrite");
    assert!(!paths.installation_state_root().join("knowledge").exists());
}

#[tokio::test]
async fn knowledge_snapshot_enforces_the_registered_archive_bound_before_publication() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_control_knowledge(&store, &paths).await;
    let session = store
        .begin_payload_snapshot(registry_with_payload_limit(1_024))
        .await
        .unwrap();
    let archive = temporary.path().join("bounded.a3s-okf-backup");

    assert_eq!(
        session
            .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 3_500)
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_backup_invalid"
    );
    assert!(!archive.exists());
}

#[tokio::test]
async fn knowledge_snapshot_rejects_payload_without_bound_control_effect_authority() {
    let temporary = TempDir::new().unwrap();
    let installation =
        InstallationId::new(InstallationKind::Workspace, "knowledge-unbound").unwrap();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_knowledge(&paths, installation).await;
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("unbound.a3s-okf-backup");

    assert_eq!(
        session
            .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive, 3_600)
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_snapshot_invalid"
    );
}

#[tokio::test]
async fn knowledge_snapshot_rejects_control_application_evidence_for_another_projection() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary, control_installation());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_control_knowledge_with_evidence(&store, &paths, false).await;
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("wrong-projection.a3s-okf-backup");

    assert_eq!(
        session
            .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 3_700)
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_snapshot_invalid"
    );
    assert!(!archive.exists());
}

#[tokio::test]
async fn knowledge_snapshot_accepts_exact_control_recorded_removal() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary, control_installation());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let promoted = seed_control_knowledge(&store, &paths).await;
    let removed = remove_control_knowledge(&store, &paths, &promoted).await;
    assert_eq!(
        removed.observation.state,
        OkfKnowledgeObservedState::Removed
    );
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("removed.a3s-okf-backup");
    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 3_800)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.retained_bindings, 1);
    assert_eq!(snapshot.manifest.selected_surfaces, 0);
    let verified = snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();
    assert_eq!(
        verified.bindings()[0].observation.state,
        removed.observation.state
    );
    assert!(verified.selected().is_empty());
}

#[tokio::test]
async fn knowledge_snapshot_reuses_one_origin_binding_across_control_reenable() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary, control_installation());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let promoted = seed_control_knowledge(&store, &paths).await;
    disable_and_reenable_control_knowledge(&store, &promoted).await;
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("reenabled.a3s-okf-backup");
    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 3_850)
        .await
        .unwrap();

    assert_eq!(snapshot.manifest.retained_bindings, 1);
    assert_eq!(snapshot.manifest.selected_surfaces, 1);
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
async fn knowledge_snapshot_rejects_payload_removed_without_a_control_effect() {
    let temporary = TempDir::new().unwrap();
    let paths = paths(&temporary, control_installation());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let promoted = seed_control_knowledge(&store, &paths).await;
    remove_knowledge_without_control(&paths, &promoted).await;
    let session = store.begin_payload_snapshot(registry()).await.unwrap();

    assert_eq!(
        session
            .snapshot_knowledge(
                OkfKnowledgeStoragePolicy::default(),
                temporary.path().join("unrecorded-remove.a3s-okf-backup"),
                3_900,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_snapshot_invalid"
    );
}

#[tokio::test]
async fn knowledge_snapshot_backend_rejects_an_exclusive_guard_for_another_root() {
    let temporary = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let installation = InstallationId::new(InstallationKind::User, "knowledge-guard").unwrap();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let adapter = SqliteOkfKnowledgeAdapter::from_extension_paths(&paths);
    let wrong_guard = StateMaintenanceLock::new(other.path())
        .acquire_exclusive()
        .await
        .unwrap();
    let archive = temporary.path().join("wrong-guard.a3s-okf-backup");

    assert_eq!(
        adapter
            .backup_if_present_under_maintenance(
                &wrong_guard,
                &installation,
                archive.clone(),
                3_750,
                16 * 1024 * 1024,
                |_| Ok(()),
            )
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_maintenance_mismatch"
    );
    assert!(!archive.exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn knowledge_snapshot_rejects_a_linked_owner_root_without_following_it() {
    let temporary = TempDir::new().unwrap();
    let external = TempDir::new().unwrap();
    std::fs::write(external.path().join("sentinel"), b"outside").unwrap();
    let installation = InstallationId::new(InstallationKind::User, "knowledge-link").unwrap();
    let paths = paths(&temporary, installation);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    crate::test_filesystem::create_directory_link(
        external.path(),
        &paths.installation_state_root().join("knowledge"),
    );
    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    let archive = temporary.path().join("linked.a3s-okf-backup");

    assert_eq!(
        session
            .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 4_000)
            .await
            .unwrap_err()
            .code,
        "use.okf.knowledge_database_path_invalid"
    );
    assert_eq!(
        std::fs::read(external.path().join("sentinel")).unwrap(),
        b"outside"
    );
    assert!(!archive.exists());
}
