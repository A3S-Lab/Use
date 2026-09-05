use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, InstallationKind};

use super::{
    ControlCapabilityDescriptorSnapshot, ControlCapabilityDescriptorSnapshotKey,
    ControlCapabilityDescriptorSnapshotRestoreVerification,
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilityPayloadRestoreCoordinator,
    ControlCapabilitySignerPolicy,
};
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;

fn installation(label: &str) -> InstallationId {
    InstallationId::new(InstallationKind::User, format!("user/{label}")).unwrap()
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn empty_snapshot(
    installation: &InstallationId,
    installation_generation: u64,
    capability_generation: u64,
    descriptor_digest: char,
) -> ControlCapabilityDescriptorSnapshot {
    let key = ControlCapabilityDescriptorSnapshotKey::new(
        installation.clone(),
        installation_generation,
        capability_generation,
        digest(descriptor_digest),
    )
    .unwrap();
    let policy =
        ControlCapabilitySignerPolicy::new(BTreeMap::<String, BTreeSet<String>>::new()).unwrap();
    ControlCapabilityDescriptorSnapshot::new(key, Vec::new(), policy).unwrap()
}

fn coordinator(
    state_root: &std::path::Path,
    installation: &InstallationId,
) -> ControlCapabilityPayloadRestoreCoordinator {
    ControlCapabilityPayloadRestoreCoordinator::new(
        CapabilityGatewayCatalogStore::new(state_root, installation.clone()).unwrap(),
        ControlCapabilityDescriptorSnapshotStore::new(state_root, installation.clone()).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn coordinator_preflights_both_owners_and_replays_exactly() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("capability-payload-coordinator");
    let coordinator = coordinator(&temporary.path().join("state"), &installation);
    let catalog = CapabilityGatewayCatalog::new(installation.clone(), 1, Vec::new()).unwrap();
    let snapshot = empty_snapshot(&installation, 1, 1, 'a');
    let plan = coordinator
        .plan_clean_restore(
            std::slice::from_ref(&catalog),
            std::slice::from_ref(&snapshot),
        )
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let first = coordinator
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&catalog),
            std::slice::from_ref(&snapshot),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap();
    assert!(first.changed);
    assert!(first.catalog.changed);
    assert!(first.descriptor_snapshot.changed);
    assert_eq!(first.plan_digest, plan_digest);
    assert_eq!(coordinator.catalog_store().list().await.unwrap().len(), 1);
    assert_eq!(
        coordinator
            .descriptor_snapshot_store()
            .keys()
            .await
            .unwrap()
            .len(),
        1
    );

    let replay = coordinator
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&catalog),
            std::slice::from_ref(&snapshot),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap();
    assert!(!replay.changed);
    assert!(!replay.catalog.changed);
    assert!(!replay.descriptor_snapshot.changed);
}

#[tokio::test]
async fn coordinator_rejects_a_second_owner_conflict_before_first_publication() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("capability-payload-coordinator-conflict");
    let state_root = temporary.path().join("state");
    let coordinator = coordinator(&state_root, &installation);
    let catalog = CapabilityGatewayCatalog::new(installation.clone(), 2, Vec::new()).unwrap();
    let existing = empty_snapshot(&installation, 1, 1, 'a');
    let requested = empty_snapshot(&installation, 2, 2, 'b');
    coordinator
        .descriptor_snapshot_store()
        .publish(&existing)
        .await
        .unwrap();
    let plan = coordinator
        .plan_clean_restore(
            std::slice::from_ref(&catalog),
            std::slice::from_ref(&requested),
        )
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let error = coordinator
        .apply_clean_restore(
            &plan,
            std::slice::from_ref(&catalog),
            std::slice::from_ref(&requested),
            &plan_digest,
            ControlCapabilityDescriptorSnapshotRestoreVerification::ProofOnly,
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_descriptor_snapshot_restore_target_not_empty"
    );
    assert!(coordinator.catalog_store().list().await.unwrap().is_empty());
    assert_eq!(
        coordinator
            .descriptor_snapshot_store()
            .keys()
            .await
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn coordinator_rejects_mismatched_owner_roots() {
    let first = installation("capability-payload-coordinator-roots");
    let second = first.clone();
    let first_store =
        CapabilityGatewayCatalogStore::new(tempfile::tempdir().unwrap().path().join("one"), first)
            .unwrap();
    let second_store = ControlCapabilityDescriptorSnapshotStore::new(
        tempfile::tempdir().unwrap().path().join("two"),
        second,
    )
    .unwrap();
    let error =
        ControlCapabilityPayloadRestoreCoordinator::new(first_store, second_store).unwrap_err();
    assert_eq!(error.code, "use.control.capability_payload_restore_invalid");
}
