use std::path::PathBuf;

use a3s_use_core::{InstallationId, InstallationKind};
use a3s_use_extension::StateMaintenanceLock;
use tempfile::TempDir;

use super::payload_owner::*;
use super::ControlStore;
use crate::okf_knowledge::{OkfKnowledgeStoragePolicy, SqliteOkfKnowledgeAdapter};

use super::payload_knowledge_tests::support::*;

struct RestoreSource {
    _temporary: TempDir,
    installation: InstallationId,
    registry: ControlPayloadOwnerRegistry,
    binding: ControlPayloadSnapshotBinding,
    control_export: Vec<u8>,
    snapshot: ControlKnowledgePayloadSnapshot,
    archive: Option<PathBuf>,
}

impl RestoreSource {
    async fn populated() -> Self {
        let temporary = TempDir::new().unwrap();
        let installation = control_installation();
        let source_paths = paths(&temporary, installation.clone());
        let store = ControlStore::from_extension_paths(&source_paths).unwrap();
        store.initialize().await.unwrap();
        seed_control_knowledge(&store, &source_paths).await;
        let registry = registry();
        let session = store
            .begin_payload_snapshot(registry.clone())
            .await
            .unwrap();
        let archive = temporary.path().join("knowledge.a3s-okf-backup");
        let snapshot = session
            .snapshot_knowledge(OkfKnowledgeStoragePolicy::default(), archive.clone(), 5_400)
            .await
            .unwrap();
        let binding = session.binding().clone();
        let control_export = session.control_export().to_vec();
        drop(session);
        Self {
            _temporary: temporary,
            installation,
            registry,
            binding,
            control_export,
            snapshot,
            archive: Some(archive),
        }
    }

    async fn absent() -> Self {
        let temporary = TempDir::new().unwrap();
        let installation =
            InstallationId::new(InstallationKind::User, "knowledge-restore-absent-layout").unwrap();
        let source_paths = paths(&temporary, installation.clone());
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
                temporary.path().join("absent.a3s-okf-backup"),
                5_450,
            )
            .await
            .unwrap();
        let binding = session.binding().clone();
        let control_export = session.control_export().to_vec();
        drop(session);
        Self {
            _temporary: temporary,
            installation,
            registry,
            binding,
            control_export,
            snapshot,
            archive: None,
        }
    }

    async fn verify(&self) -> VerifiedControlKnowledgePayloadSnapshot {
        self.snapshot
            .verify_offline(
                &self.registry,
                &self.binding,
                &self.control_export,
                self.archive.clone(),
            )
            .await
            .unwrap()
    }
}

fn staging(paths: &a3s_use_extension::ExtensionPaths) -> PathBuf {
    paths
        .installation_state_root()
        .join("operations/state-restores/control-fixture/knowledge")
}

#[tokio::test]
async fn knowledge_restore_stage_recovers_an_exact_completed_partial() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staging = staging(&target_paths);
    let first = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging.clone(),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let candidate = first.candidate_path().unwrap().to_path_buf();
    let partial = staging.join("knowledge.sqlite3.partial");
    std::fs::rename(&candidate, &partial).unwrap();

    let replay = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging,
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();

    assert_eq!(replay.candidate_path(), Some(candidate.as_path()));
    assert!(candidate.is_file());
    assert!(!partial.exists());
}

#[tokio::test]
async fn knowledge_restore_activation_recovers_after_publish_before_result() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let candidate = staged.candidate_path().unwrap().to_path_buf();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let adapter = SqliteOkfKnowledgeAdapter::from_extension_paths(&target_paths);
    let database = adapter
        .restore_database_guard(&source.installation)
        .await
        .unwrap();
    tokio::fs::rename(&candidate, database.path())
        .await
        .unwrap();
    drop(database);

    let result = staged.activate(&maintenance).await.unwrap();

    result.validate(&source.registry).unwrap();
    assert!(matches!(
        result.payload,
        ControlKnowledgePayloadRestoreState::Database { .. }
    ));
    assert!(!candidate.exists());
}

#[tokio::test]
async fn knowledge_restore_activation_reclaims_an_exact_empty_live_layout() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let adapter = SqliteOkfKnowledgeAdapter::from_extension_paths(&target_paths);
    let empty_layout = adapter
        .restore_database_guard(&source.installation)
        .await
        .unwrap();
    assert!(!empty_layout.path().exists());
    drop(empty_layout);
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    let result = staged.activate(&maintenance).await.unwrap();

    assert!(matches!(
        result.payload,
        ControlKnowledgePayloadRestoreState::Database { .. }
    ));
    assert!(adapter
        .scope_directory(&source.installation)
        .unwrap()
        .join("knowledge.sqlite3")
        .is_file());
}

#[tokio::test]
async fn knowledge_restore_activation_rejects_an_exclusive_guard_for_another_root() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let other = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let candidate = staged.candidate_path().unwrap().to_path_buf();
    let wrong = StateMaintenanceLock::new(other.path())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&wrong).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert!(candidate.is_file());
    assert!(!target_paths
        .installation_state_root()
        .join("knowledge")
        .exists());
}

#[tokio::test]
async fn knowledge_restore_stage_rejects_a_different_target_storage_policy() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staging = staging(&target_paths);
    let different_policy = OkfKnowledgeStoragePolicy::new(1, 1, 1, 1).unwrap();

    assert_eq!(
        verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                staging.clone(),
                different_policy,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert!(!staging.join("knowledge.sqlite3").exists());
    assert!(!target_paths
        .installation_state_root()
        .join("knowledge")
        .exists());
}

#[tokio::test]
async fn knowledge_restore_stage_rejects_the_live_payload_root() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let live_root = target_paths.installation_state_root().join("knowledge");

    assert_eq!(
        verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                live_root.clone(),
                OkfKnowledgeStoragePolicy::default(),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert!(!live_root.exists());
}

#[tokio::test]
async fn knowledge_restore_activation_rejects_unowned_live_payload_entries() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let unexpected = target_paths
        .installation_state_root()
        .join("knowledge/unowned");
    std::fs::create_dir_all(unexpected.parent().unwrap()).unwrap();
    std::fs::write(&unexpected, b"preserve-me").unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_target_not_empty"
    );
    assert_eq!(std::fs::read(&unexpected).unwrap(), b"preserve-me");
    assert!(staged.candidate_path().unwrap().is_file());
}

#[tokio::test]
async fn absent_knowledge_restore_rejects_an_existing_live_payload_root() {
    let source = RestoreSource::absent().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let live_root = target_paths.installation_state_root().join("knowledge");
    std::fs::create_dir_all(&live_root).unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_target_not_empty"
    );
    assert!(live_root.is_dir());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn knowledge_restore_stage_rejects_a_linked_directory_without_following_it() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let restore_root = target_paths
        .installation_state_root()
        .join("operations/state-restores");
    std::fs::create_dir_all(&restore_root).unwrap();
    crate::test_filesystem::create_directory_link(
        outside.path(),
        &restore_root.join("control-fixture"),
    );

    assert_eq!(
        verified
            .stage_clean_restore(
                target_paths.installation_state_root(),
                staging(&target_paths),
                OkfKnowledgeStoragePolicy::default(),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert_eq!(
        std::fs::read(outside.path().join("sentinel")).unwrap(),
        b"outside"
    );
    assert!(!outside.path().join("knowledge.sqlite3").exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn knowledge_restore_activation_rejects_a_linked_live_payload_root() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let outside = TempDir::new().unwrap();
    std::fs::write(outside.path().join("sentinel"), b"outside").unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    crate::test_filesystem::create_directory_link(
        outside.path(),
        &target_paths.installation_state_root().join("knowledge"),
    );
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();

    assert_eq!(
        staged.activate(&maintenance).await.unwrap_err().code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
    assert_eq!(
        std::fs::read(outside.path().join("sentinel")).unwrap(),
        b"outside"
    );
    assert!(!outside.path().join("sqlite").exists());
    assert!(staged.candidate_path().unwrap().is_file());
}

#[tokio::test]
async fn knowledge_restore_result_rejects_canonical_evidence_tampering() {
    let source = RestoreSource::populated().await;
    let verified = source.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, source.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let result = staged.activate(&maintenance).await.unwrap();
    let mut json = serde_json::to_value(&result).unwrap();
    json["inventoryDigest"] = serde_json::Value::String(digest('f'));
    let tampered: ControlKnowledgePayloadRestoreResult = serde_json::from_value(json).unwrap();

    assert_eq!(
        tampered.validate(&source.registry).unwrap_err().code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
}

#[tokio::test]
async fn knowledge_restore_result_rejects_a_different_valid_snapshot() {
    let expected = RestoreSource::populated().await;
    let foreign = RestoreSource::absent().await;
    let verified = foreign.verify().await;
    let target = TempDir::new().unwrap();
    let target_paths = paths(&target, foreign.installation.clone());
    let staged = verified
        .stage_clean_restore(
            target_paths.installation_state_root(),
            staging(&target_paths),
            OkfKnowledgeStoragePolicy::default(),
        )
        .await
        .unwrap();
    let maintenance = StateMaintenanceLock::new(target_paths.installation_state_root())
        .acquire_exclusive()
        .await
        .unwrap();
    let result = staged.activate(&maintenance).await.unwrap();

    result.validate(&expected.registry).unwrap();
    assert_eq!(
        result
            .validate_for_snapshot(&expected.registry, &expected.snapshot)
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_restore_invalid"
    );
}
