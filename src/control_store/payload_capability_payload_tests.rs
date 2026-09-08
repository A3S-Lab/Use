use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::CapabilityGatewayCatalog;
use tempfile::TempDir;

use super::aggregate_tests::fixtures::control_installation;
use super::effect_owner::capability_plane::{
    ControlCapabilityDescriptorSnapshot, ControlCapabilityDescriptorSnapshotKey,
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilitySignerPolicy,
};
use super::payload_installation_restore_staging_tests::populated_snapshot;
use super::payload_installation_snapshot_tests::{paths, registry};
use super::payload_owner::*;
use super::ControlStore;
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;
use crate::okf_knowledge::OkfKnowledgeStoragePolicy;

#[tokio::test]
async fn capability_payload_is_snapshotted_staged_and_activated_with_the_complete_set() {
    let verified = populated_snapshot(15_100).await;
    let capability_snapshot = &verified.manifest().capability_payload;
    assert!(matches!(
        capability_snapshot.manifest.payload,
        ControlCapabilityPayloadState::Absent
    ));
    assert!(capability_snapshot.manifest.entries.is_empty());
    assert_eq!(capability_snapshot.receipt.file_count, 0);

    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(staged.capability_payload_candidate_path().is_none());

    let result = staged.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 7);
    assert!(!state_root.join("capability-gateway").exists());
}

#[tokio::test]
async fn seeded_capability_payload_archive_round_trips_through_the_complete_set() {
    let source = TempDir::new().unwrap();
    let installation = control_installation();
    let source_paths = paths(&source);
    let store = ControlStore::from_extension_paths(&source_paths).unwrap();
    store.initialize().await.unwrap();

    let catalog = CapabilityGatewayCatalog::new(installation.clone(), 1, Vec::new()).unwrap();
    let catalog_digest = catalog.descriptor_digest().unwrap();
    CapabilityGatewayCatalogStore::from_extension_paths(&source_paths)
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
    ControlCapabilityDescriptorSnapshotStore::from_extension_paths(&source_paths)
        .publish(&descriptor_snapshot)
        .await
        .unwrap();

    let archive = source.path().join("seeded.complete-snapshot");
    let registry = registry();
    let session = store
        .begin_payload_snapshot(registry.clone())
        .await
        .unwrap();
    session
        .snapshot_complete_set(
            archive.clone(),
            OkfKnowledgeStoragePolicy::default(),
            16_000,
        )
        .await
        .unwrap();
    drop(session);

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

    let target = TempDir::new().unwrap();
    let state_root = target.path().join("state");
    let staged = verified
        .stage_clean_restore(state_root.clone(), OkfKnowledgeStoragePolicy::default())
        .await
        .unwrap();
    assert!(staged
        .capability_payload_candidate_path()
        .is_some_and(|path| path.is_dir()));

    let result = staged.activate().await.unwrap();
    assert_eq!(result.checkpoint_count_for_test(), 7);
    // Release the exclusive restore fence before shared live-store reads.
    drop(staged);

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

#[test]
fn capability_payload_restore_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<StagedControlInstallationRestore>();
    assert_send_sync::<ControlCapabilityPayloadEntry>();
    assert_send_sync::<ControlCapabilityPayloadEntryKind>();
    assert_send_sync::<ControlCapabilityPayloadRestoreResult>();
    assert_send_sync::<ControlCapabilityPayloadRestoreState>();
    assert_send_sync::<ControlCapabilityPayloadSnapshot>();
    assert_send_sync::<VerifiedControlCapabilityPayloadSnapshot>();
}
