use std::sync::Arc;

use a3s_use_core::{
    inspect_okf_bundle_files, InstallationId, InstallationKind, OkfBundleContract, OkfBundleFile,
    OkfBundleLimits, OkfFormatVersion, PlanQualifiedSurfaceRef, PluginSurfaceKind,
    PluginSurfaceRef, OKF_BUNDLE_CONTRACT_SCHEMA,
};
use a3s_use_extension::{ExtensionPaths, StateMaintenanceLock};
use tempfile::TempDir;

use super::payload_owner::*;
use super::ControlStore;
use crate::okf_knowledge::{
    OkfKnowledgeClient, OkfKnowledgeStageRequest, OkfKnowledgeStageSpec, OkfKnowledgeStoragePolicy,
    SqliteOkfKnowledgeAdapter,
};

#[tokio::test]
async fn knowledge_snapshot_is_control_bound_path_free_and_offline_verified() {
    let temporary = TempDir::new().unwrap();
    let installation =
        InstallationId::new(InstallationKind::Workspace, "knowledge-snapshot").unwrap();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_knowledge(&paths, installation.clone()).await;

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
        .verify_offline(&registry, session.binding(), Some(archive.clone()))
        .await
        .unwrap();
    assert_eq!(verified.bindings().len(), 1);
    assert_eq!(verified.selected().len(), 1);
    assert_eq!(
        snapshot
            .verify_offline(&registry, session.binding(), None)
            .await
            .unwrap_err()
            .code,
        "use.control_store.knowledge_payload_snapshot_invalid"
    );

    let decoded: ControlKnowledgePayloadSnapshot =
        serde_json::from_slice(&serde_json::to_vec(&snapshot).unwrap()).unwrap();
    decoded
        .verify_offline(&registry, session.binding(), Some(archive.clone()))
        .await
        .unwrap();
    let mut rebound_json = serde_json::to_value(&snapshot).unwrap();
    rebound_json["manifest"]["binding"]["controlExportDigest"] =
        serde_json::Value::String(digest('f'));
    let rebound: ControlKnowledgePayloadSnapshot = serde_json::from_value(rebound_json).unwrap();
    assert_eq!(
        rebound
            .verify_offline(&registry, session.binding(), Some(archive.clone()))
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
            .verify_offline(&registry, session.binding(), Some(archive))
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
        .verify_offline(&registry, session.binding(), None)
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
    let installation =
        InstallationId::new(InstallationKind::Workspace, "knowledge-bounded").unwrap();
    let paths = paths(&temporary, installation.clone());
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    seed_knowledge(&paths, installation).await;
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

async fn seed_knowledge(paths: &ExtensionPaths, installation: InstallationId) {
    let files = vec![OkfBundleFile::new(
        "concept.md",
        b"---\ntype: Metric\n---\n\n# Throughput\n",
    )];
    let limits = OkfBundleLimits::default();
    let inspection =
        inspect_okf_bundle_files(OkfFormatVersion::V0_2, limits.clone(), &files).unwrap();
    let bundle = OkfBundleContract {
        schema: OKF_BUNDLE_CONTRACT_SCHEMA.to_string(),
        format_version: inspection.format_version,
        root: "knowledge".to_string(),
        content_digest: inspection.content_digest,
        concept_count: inspection.concept_count,
        file_count: inspection.file_count,
        expanded_bytes: inspection.expanded_bytes,
        limits,
    };
    let spec = OkfKnowledgeStageSpec {
        operation_id: "knowledge-snapshot-operation".to_string(),
        scope: installation,
        surface: PlanQualifiedSurfaceRef {
            package_id: "acme/research".to_string(),
            surface: PluginSurfaceRef {
                kind: PluginSurfaceKind::Okf,
                id: "domain-knowledge".to_string(),
            },
        },
        generation: 1,
        package_digest: digest('a'),
        manifest_digest: digest('b'),
        bundle,
    };
    let adapter = Arc::new(SqliteOkfKnowledgeAdapter::from_extension_paths(paths));
    let client = OkfKnowledgeClient::new(adapter);
    let staged = client
        .stage(OkfKnowledgeStageRequest::new(spec, files).unwrap())
        .await
        .unwrap();
    client.promote(&staged.receipt).await.unwrap();
}

fn paths(temporary: &TempDir, installation: InstallationId) -> ExtensionPaths {
    ExtensionPaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
        installation,
    )
    .unwrap()
}

fn registry() -> ControlPayloadOwnerRegistry {
    registry_with_payload_limit(16 * 1024 * 1024)
}

fn registry_with_payload_limit(max_payload_bytes: u64) -> ControlPayloadOwnerRegistry {
    ControlPayloadOwnerRegistry::new(
        ControlPayloadOwnerId::ALL
            .into_iter()
            .map(|owner| {
                if owner == ControlPayloadOwnerId::ArtifactStore {
                    ControlPayloadOwnerRegistration::excluded_global(owner).unwrap()
                } else {
                    let schema = if owner == ControlPayloadOwnerId::KnowledgePayload {
                        CONTROL_KNOWLEDGE_PAYLOAD_SNAPSHOT_SCHEMA.to_string()
                    } else {
                        format!("a3s.use.test.{}-snapshot.v1", owner.as_str())
                    };
                    ControlPayloadOwnerRegistration::snapshotted(
                        owner,
                        schema,
                        ControlPayloadOwnerLimits::new(16, max_payload_bytes, 256 * 1024).unwrap(),
                    )
                    .unwrap()
                }
            })
            .collect(),
    )
    .unwrap()
}

fn digest(seed: char) -> String {
    format!("sha256:{}", seed.to_string().repeat(64))
}
