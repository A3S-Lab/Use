use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId};
use a3s_use_extension::ExtensionPaths;
use tempfile::TempDir;

use super::aggregate_tests::fixtures::control_installation;
use super::effect_owner::capability_plane::{
    ControlCapabilityDescriptorSnapshot, ControlCapabilityDescriptorSnapshotKey,
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilitySignerPolicy,
};
use super::payload_installation_snapshot_tests::{paths, registry};
use super::payload_owner::*;
use super::ControlStore;
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

const INDEX_MARKER: &[u8] = b"CAPABILITY-INDEX-MARKER-MUST-NOT-SNAPSHOT";
const LEASE_MARKER: &[u8] = b"GENERATION-LEASE-MARKER-MUST-NOT-SNAPSHOT";

#[tokio::test]
async fn operational_index_and_leases_are_excluded_from_capability_complete_set() {
    let source = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&source);
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();
    let (catalog_digest, descriptor_digest, descriptor_key) =
        seed_capability_archive(&source_paths, &installation).await;
    plant_operational_index_and_leases(source_paths.state_root());

    let archive = source.path().join("exclude-ops.complete-snapshot");
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    session
        .snapshot_complete_set(
            archive.clone(),
            OkfKnowledgeStoragePolicy::default(),
            17_000,
        )
        .await
        .unwrap();
    drop(session);

    let archive_bytes = std::fs::read(&archive).unwrap();
    assert!(
        !contains_bytes(&archive_bytes, INDEX_MARKER),
        "complete-set archive must exclude capability-index bytes"
    );
    assert!(
        !contains_bytes(&archive_bytes, LEASE_MARKER),
        "complete-set archive must exclude generation-leases bytes"
    );

    let verified = VerifiedControlInstallationSnapshot::verify_offline(registry, archive)
        .await
        .unwrap();
    let capability = &verified.manifest().capability_payload;
    assert!(matches!(
        capability.manifest.payload,
        ControlCapabilityPayloadState::Archive { .. }
    ));
    assert_eq!(capability.receipt.file_count, 2);
    assert_eq!(capability.manifest.entries.len(), 2);
    assert!(capability.manifest.entries.iter().any(|entry| {
        entry.kind == ControlCapabilityPayloadEntryKind::Catalog && entry.digest == catalog_digest
    }));
    assert!(capability.manifest.entries.iter().any(|entry| {
        entry.kind == ControlCapabilityPayloadEntryKind::DescriptorSnapshot
            && entry.digest == descriptor_digest
    }));
    let encoded = serde_json::to_string(capability).unwrap();
    assert!(!encoded.contains("capability-index"));
    assert!(!encoded.contains("generation-leases"));
    assert!(!encoded.contains(std::str::from_utf8(INDEX_MARKER).unwrap()));
    assert!(!encoded.contains(std::str::from_utf8(LEASE_MARKER).unwrap()));

    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    let result = staged.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 7);
    drop(staged);

    assert!(!state_root.join("capability-index").exists());
    assert!(!state_root.join("generation-leases").exists());
    let restored_catalogs =
        CapabilityGatewayCatalogStore::new(state_root.clone(), installation.clone())
            .unwrap()
            .list()
            .await
            .unwrap();
    assert_eq!(restored_catalogs.len(), 1);
    assert_eq!(restored_catalogs[0].digest, catalog_digest);
    let restored = ControlCapabilityDescriptorSnapshotStore::new(state_root, installation)
        .unwrap()
        .get(&descriptor_key)
        .await
        .unwrap()
        .expect("restored descriptor snapshot");
    assert_eq!(restored.digest().unwrap(), descriptor_digest);
}

#[tokio::test]
async fn capability_archive_length_and_manifest_tampering_fail_closed() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let _ = seed_capability_archive(&paths, &installation).await;

    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    let archive = temporary.path().join("capability.archive");
    let snapshot = session
        .snapshot_capability_payload(archive.clone(), 18_000)
        .await
        .unwrap();
    assert!(matches!(
        snapshot.manifest.payload,
        ControlCapabilityPayloadState::Archive { .. }
    ));

    let original = std::fs::read(&archive).unwrap();
    let mut trailing = original.clone();
    trailing.push(0);
    std::fs::write(&archive, &trailing).unwrap();
    assert_eq!(
        snapshot
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.capability_payload_snapshot_invalid"
    );

    std::fs::write(&archive, &original[..original.len() - 1]).unwrap();
    assert_eq!(
        snapshot
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive.clone()),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.capability_payload_snapshot_invalid"
    );
    std::fs::write(&archive, &original).unwrap();

    let mut rebound = snapshot.clone();
    rebound.manifest.entries[0].sha256 = format!("sha256:{}", "b".repeat(64));
    assert_eq!(
        rebound
            .verify_offline(
                &registry,
                session.binding(),
                session.control_export(),
                Some(archive),
            )
            .await
            .unwrap_err()
            .code,
        "use.control_store.capability_payload_snapshot_invalid"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn capability_snapshot_rejects_a_linked_live_root() {
    let temporary = TempDir::new().unwrap();
    let installation = control_installation();
    let paths = paths(&temporary);
    let store = ControlStore::from_extension_paths(&paths).unwrap();
    store.initialize().await.unwrap();
    let _ = seed_capability_archive(&paths, &installation).await;

    let live = paths.state_root().join("capability-gateway");
    let relocated = temporary.path().join("capability-gateway.relocated");
    std::fs::rename(&live, &relocated).unwrap();
    std::os::unix::fs::symlink(&relocated, &live).unwrap();

    let session = store.begin_payload_snapshot(registry()).await.unwrap();
    assert_eq!(
        session
            .snapshot_capability_payload(temporary.path().join("linked.archive"), 18_100)
            .await
            .unwrap_err()
            .code,
        "use.control_store.capability_payload_snapshot_invalid"
    );
}

async fn seed_capability_archive(
    paths: &ExtensionPaths,
    installation: &InstallationId,
) -> (String, String, ControlCapabilityDescriptorSnapshotKey) {
    let catalog = CapabilityGatewayCatalog::new(installation.clone(), 1, Vec::new()).unwrap();
    let catalog_digest = catalog.descriptor_digest().unwrap();
    CapabilityGatewayCatalogStore::from_extension_paths(paths)
        .publish(&catalog)
        .await
        .unwrap();

    let descriptor_key = ControlCapabilityDescriptorSnapshotKey::new(
        installation.clone(),
        1,
        1,
        format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap();
    let signer_policy =
        ControlCapabilitySignerPolicy::new(BTreeMap::<String, BTreeSet<String>>::new()).unwrap();
    let descriptor_snapshot =
        ControlCapabilityDescriptorSnapshot::new(descriptor_key.clone(), Vec::new(), signer_policy)
            .unwrap();
    let descriptor_digest = descriptor_snapshot.digest().unwrap();
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(paths)
        .publish(&descriptor_snapshot)
        .await
        .unwrap();
    (catalog_digest, descriptor_digest, descriptor_key)
}

fn plant_operational_index_and_leases(state_root: &Path) {
    let index = state_root.join("capability-index/sha256/aa/index.json");
    std::fs::create_dir_all(index.parent().unwrap()).unwrap();
    std::fs::write(index, INDEX_MARKER).unwrap();

    let lease = state_root.join("generation-leases/acme/fixture/00000000000000000001.lock");
    std::fs::create_dir_all(lease.parent().unwrap()).unwrap();
    std::fs::write(lease, LEASE_MARKER).unwrap();
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}
