use a3s_use_extension::ExtensionPaths;
use tempfile::TempDir;

use super::aggregate_tests::fixtures::control_installation;
use super::payload_installation_snapshot_tests::registry;
use super::payload_owner::*;
use super::ControlStore;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[tokio::test]
async fn complete_snapshot_never_overwrites_an_existing_destination() {
    let fixture = Fixture::new().await;
    let destination = fixture.temporary.path().join("existing.snapshot");
    std::fs::write(&destination, b"sentinel").unwrap();

    assert_eq!(
        fixture
            .session
            .snapshot_complete_set(
                destination.clone(),
                OkfKnowledgeStoragePolicy::default(),
                5_000,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_snapshot_exists"
    );
    assert_eq!(std::fs::read(destination).unwrap(), b"sentinel");
}

#[tokio::test]
async fn complete_snapshot_rejects_an_in_state_destination() {
    let fixture = Fixture::new().await;
    let destination = fixture
        .paths
        .installation_state_root()
        .join("backup.snapshot");

    assert_eq!(
        fixture
            .session
            .snapshot_complete_set(
                destination.clone(),
                OkfKnowledgeStoragePolicy::default(),
                5_001,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_snapshot_path_invalid"
    );
    assert!(!destination.exists());
}

#[tokio::test]
async fn complete_snapshot_rejects_every_use_owned_root() {
    let fixture = Fixture::new().await;
    let parent = fixture.paths.use_paths().data_root().join("backups");
    std::fs::create_dir_all(&parent).unwrap();
    let destination = parent.join("backup.snapshot");

    assert_eq!(
        fixture
            .session
            .snapshot_complete_set(
                destination.clone(),
                OkfKnowledgeStoragePolicy::default(),
                5_002,
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.complete_snapshot_path_invalid"
    );
    assert!(!destination.exists());
}

#[tokio::test]
async fn offline_verification_rejects_truncation_tampering_and_trailing_bytes() {
    for case in ["truncated", "tampered", "trailing"] {
        let fixture = Fixture::new().await;
        let destination = fixture.temporary.path().join(format!("{case}.snapshot"));
        fixture
            .session
            .snapshot_complete_set(
                destination.clone(),
                OkfKnowledgeStoragePolicy::default(),
                5_100,
            )
            .await
            .unwrap();
        let mut bytes = std::fs::read(&destination).unwrap();
        match case {
            "truncated" => {
                bytes.pop();
            }
            "tampered" => {
                let index = bytes.len() - 1;
                bytes[index] ^= 1;
            }
            "trailing" => bytes.push(0),
            _ => unreachable!(),
        }
        std::fs::write(&destination, bytes).unwrap();

        assert_eq!(
            VerifiedControlInstallationSnapshot::verify_offline(
                fixture.registry.clone(),
                destination,
            )
            .await
            .unwrap_err()
            .code,
            "use.control_store.complete_snapshot_invalid",
            "case {case} was accepted"
        );
    }
}

#[cfg(unix)]
#[tokio::test]
async fn offline_verification_rejects_a_linked_archive() {
    let fixture = Fixture::new().await;
    let archive = fixture.temporary.path().join("source.snapshot");
    fixture
        .session
        .snapshot_complete_set(archive.clone(), OkfKnowledgeStoragePolicy::default(), 5_200)
        .await
        .unwrap();
    let linked = fixture.temporary.path().join("linked.snapshot");
    std::os::unix::fs::symlink(&archive, &linked).unwrap();

    assert_eq!(
        VerifiedControlInstallationSnapshot::verify_offline(
            fixture.registry.clone(),
            linked.clone(),
        )
        .await
        .unwrap_err()
        .code,
        "use.control_store.complete_snapshot_invalid"
    );
    std::fs::remove_file(&linked).unwrap();
}

struct Fixture {
    temporary: TempDir,
    paths: ExtensionPaths,
    registry: ControlPayloadOwnerRegistry,
    session: ControlPayloadSnapshotSession,
}

impl Fixture {
    async fn new() -> Self {
        let temporary = TempDir::new().unwrap();
        let paths = ExtensionPaths::new(
            temporary.path().join("data"),
            temporary.path().join("state"),
            control_installation(),
        )
        .unwrap();
        let store = ControlStore::from_extension_paths(&paths).unwrap();
        store.initialize().await.unwrap();
        let registry = registry();
        let session = store
            .begin_payload_snapshot(registry.clone())
            .await
            .unwrap();
        Self {
            temporary,
            paths,
            registry,
            session,
        }
    }
}
