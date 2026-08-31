use std::sync::Arc;

use a3s_use_core::{InstallationId, InstallationKind, OkfKnowledgeObservedState};
use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_owner::*;
use super::ControlStore;
use crate::okf_knowledge::{
    OkfKnowledgeClient, OkfKnowledgeStoragePolicy, SqliteOkfKnowledgeAdapter,
};

use super::payload_knowledge_tests::support::*;

#[tokio::test]
async fn verified_knowledge_restore_stages_then_activates_one_exact_database() {
    let source = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&source, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let promoted = seed_control_knowledge(&store, &source_paths).await;
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = source.path().join("knowledge.a3s-okf-backup");
    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 5_000)
        .await
        .unwrap();
    let binding = session.binding().clone();
    let control_export = session.control_export().to_vec();
    drop(session);

    let verified = snapshot
        .verify_offline(&registry, &binding, &control_export, Some(archive))
        .await
        .unwrap();
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, installation.clone());
    let staging = target_paths
        .installation_state_root()
        .join("operations/state-restores/control-fixture/knowledge");
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging,
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();

    assert!(staged.candidate_path().unwrap().is_file());
    assert!(
        !SqliteOkfKnowledgeAdapter::from_extension_paths(&target_paths)
            .scope_directory(&installation)
            .unwrap()
            .join("knowledge.sqlite3")
            .exists()
    );

    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let first = staged.activate(&maintenance).await.unwrap();
    first.validate(&registry).unwrap();
    first.validate_for_snapshot(&registry, &snapshot).unwrap();
    assert!(matches!(
        first.payload,
        ControlKnowledgePayloadRestoreState::Database { .. }
    ));
    let replay = staged.activate(&maintenance).await.unwrap();
    assert_eq!(replay, first);
    drop(maintenance);

    let restored = OkfKnowledgeClient::new(Arc::new(
        SqliteOkfKnowledgeAdapter::from_extension_paths(&target_paths),
    ))
    .observe(&promoted.receipt)
    .await
    .unwrap();
    assert_eq!(
        restored.observation.state,
        OkfKnowledgeObservedState::Promoted
    );
    assert_eq!(restored.observation, promoted.observation);
}

#[tokio::test]
async fn knowledge_restore_refuses_a_nonempty_target_without_changing_it() {
    let source = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&source, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    seed_control_knowledge(&store, &source_paths).await;
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = source.path().join("knowledge.a3s-okf-backup");
    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 5_100)
        .await
        .unwrap();
    let verified = snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();

    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, installation.clone());
    seed_knowledge(&target_paths, installation.clone()).await;
    let target_adapter = SqliteOkfKnowledgeAdapter::from_extension_paths(&target_paths);
    let before = target_adapter
        .database_file_evidence(&installation)
        .await
        .unwrap();
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            target_paths
                .installation_state_root()
                .join("operations/state-restores/control-fixture/knowledge"),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_target_not_empty"
    );
    drop(maintenance);
    assert_eq!(
        target_adapter
            .database_file_evidence(&installation)
            .await
            .unwrap(),
        before
    );
}

#[tokio::test]
async fn knowledge_restore_rejects_candidate_tampering_before_publication() {
    let source = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&source, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    seed_control_knowledge(&store, &source_paths).await;
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = source.path().join("knowledge.a3s-okf-backup");
    let snapshot = session
        .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 5_200)
        .await
        .unwrap();
    let verified = snapshot
        .verify_offline(
            &registry,
            session.binding(),
            session.control_export(),
            Some(archive),
        )
        .await
        .unwrap();
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            target_paths
                .installation_state_root()
                .join("operations/state-restores/control-fixture/knowledge"),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    std::fs::write(staged.candidate_path().unwrap(), b"tampered").unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert!(
        !SqliteOkfKnowledgeAdapter::from_extension_paths(&target_paths)
            .scope_directory(&installation)
            .unwrap()
            .join("knowledge.sqlite3")
            .exists()
    );
}

#[tokio::test]
async fn absent_knowledge_restore_is_staged_and_activated_without_creating_payload_state() {
    let source = TempDir::new().unwrap();
    let installation =
        InstallationId::new(InstallationKind::User, "knowledge-restore-absent").unwrap();
    let source_paths = paths(&source, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let snapshot = session
        .snapshot_knowledge(
            OkfKnowledgeStoragePolicy::default(),
            source.path().join("absent.a3s-okf-backup"),
            5_300,
        )
        .await
        .unwrap();
    let verified = snapshot
        .verify_offline(&registry, session.binding(), session.control_export(), None)
        .await
        .unwrap();
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, installation);
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            target_paths
                .installation_state_root()
                .join("operations/state-restores/control-fixture/knowledge"),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    assert!(staged.candidate_path().is_none());
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let result = staged.activate(&maintenance).await.unwrap();

    assert!(matches!(
        result.payload,
        ControlKnowledgePayloadRestoreState::Absent
    ));
    assert!(!target_paths
        .installation_state_root()
        .join("knowledge")
        .exists());
}

#[tokio::test]
async fn absent_knowledge_restore_rejects_unexpected_staged_database_bytes() {
    let source = TempDir::new().unwrap();
    let installation =
        InstallationId::new(InstallationKind::User, "knowledge-restore-empty-stage").unwrap();
    let source_paths = paths(&source, installation.clone());
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let snapshot = session
        .snapshot_knowledge(
            OkfKnowledgeStoragePolicy::default(),
            source.path().join("absent.a3s-okf-backup"),
            5_350,
        )
        .await
        .unwrap();
    let verified = snapshot
        .verify_offline(&registry, session.binding(), session.control_export(), None)
        .await
        .unwrap();
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, installation);
    let staging = target_paths
        .installation_state_root()
        .join("operations/state-restores/control-fixture/knowledge");
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging.clone(),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    std::fs::write(staging.join("knowledge.sqlite3"), b"unexpected").unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert!(!target_paths
        .installation_state_root()
        .join("knowledge")
        .exists());
}
