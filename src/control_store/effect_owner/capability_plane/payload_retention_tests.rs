use std::collections::{BTreeMap, BTreeSet};

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, InstallationKind};
use tokio::io::AsyncWriteExt;

use super::payload_retention::seed_test_journal;
use super::{
    ControlCapabilityDescriptorSnapshot, ControlCapabilityDescriptorSnapshotKey,
    ControlCapabilityDescriptorSnapshotStore, ControlCapabilityPayloadRetentionCoordinator,
    ControlCapabilitySignerPolicy,
};
use crate::capability_catalog_store::CapabilityGatewayCatalogStore;

fn installation(label: &str) -> InstallationId {
    InstallationId::new(InstallationKind::User, format!("user/{label}")).unwrap()
}

fn digest(byte: char) -> String {
    format!("sha256:{}", byte.to_string().repeat(64))
}

fn catalog(installation: &InstallationId, generation: u64) -> CapabilityGatewayCatalog {
    CapabilityGatewayCatalog::new(installation.clone(), generation, Vec::new()).unwrap()
}

fn snapshot(
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
) -> ControlCapabilityPayloadRetentionCoordinator {
    ControlCapabilityPayloadRetentionCoordinator::new(
        CapabilityGatewayCatalogStore::new(state_root, installation.clone()).unwrap(),
        ControlCapabilityDescriptorSnapshotStore::new(state_root, installation.clone()).unwrap(),
    )
    .unwrap()
}

#[tokio::test]
async fn coordinator_preflights_and_replays_both_retention_owners() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("capability-payload-retention");
    let state_root = temporary.path().join("state");
    let coordinator = coordinator(&state_root, &installation);
    let catalog_one = catalog(&installation, 1);
    let catalog_two = catalog(&installation, 2);
    let snapshot_one = snapshot(&installation, 1, 1, 'a');
    let snapshot_two = snapshot(&installation, 2, 2, 'b');
    let catalog_store = coordinator.catalog_store();
    catalog_store.publish(&catalog_one).await.unwrap();
    catalog_store.publish(&catalog_two).await.unwrap();
    let snapshot_store = coordinator.descriptor_snapshot_store();
    snapshot_store.publish(&snapshot_one).await.unwrap();
    snapshot_store.publish(&snapshot_two).await.unwrap();
    let catalog_two_digest = catalog_two.descriptor_digest().unwrap();
    let snapshot_two_digest = snapshot_two.digest().unwrap();

    let plan = coordinator
        .plan_retention(
            std::slice::from_ref(&catalog_two_digest),
            std::slice::from_ref(&snapshot_two_digest),
        )
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let first = coordinator
        .apply_retention(&plan, &plan_digest)
        .await
        .unwrap();
    assert!(first.changed);
    assert!(first.catalog.changed);
    assert!(first.descriptor_snapshot.changed);
    assert_eq!(first.catalog.removed.len(), 1);
    assert_eq!(first.descriptor_snapshot.removed.len(), 1);
    assert_eq!(catalog_store.list().await.unwrap().len(), 1);
    assert_eq!(snapshot_store.keys().await.unwrap().len(), 1);

    let replay = coordinator
        .apply_retention(&plan, &plan_digest)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert!(!replay.catalog.changed);
    assert!(!replay.descriptor_snapshot.changed);
}

#[tokio::test]
async fn coordinator_rejects_second_owner_drift_before_first_owner_unlink() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("capability-payload-retention-conflict");
    let state_root = temporary.path().join("state");
    let coordinator = coordinator(&state_root, &installation);
    let catalog_one = catalog(&installation, 1);
    let catalog_two = catalog(&installation, 2);
    let snapshot_one = snapshot(&installation, 1, 1, 'a');
    let snapshot_two = snapshot(&installation, 2, 2, 'b');
    coordinator
        .catalog_store()
        .publish(&catalog_one)
        .await
        .unwrap();
    coordinator
        .catalog_store()
        .publish(&catalog_two)
        .await
        .unwrap();
    coordinator
        .descriptor_snapshot_store()
        .publish(&snapshot_one)
        .await
        .unwrap();
    coordinator
        .descriptor_snapshot_store()
        .publish(&snapshot_two)
        .await
        .unwrap();
    let catalog_two_digest = catalog_two.descriptor_digest().unwrap();
    let snapshot_two_digest = snapshot_two.digest().unwrap();

    let plan = coordinator
        .plan_retention(
            std::slice::from_ref(&catalog_two_digest),
            std::slice::from_ref(&snapshot_two_digest),
        )
        .await
        .unwrap();
    // Drift only the second owner after review. Its preflight must fail while
    // the catalog still contains both records.
    let snapshot_three = snapshot(&installation, 3, 3, 'c');
    coordinator
        .descriptor_snapshot_store()
        .publish(&snapshot_three)
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let error = coordinator
        .apply_retention(&plan, &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_descriptor_snapshot_retention_stale"
    );
    assert_eq!(coordinator.catalog_store().list().await.unwrap().len(), 2);
}

#[tokio::test]
async fn coordinator_recovers_after_catalog_phase_was_durably_completed() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("capability-payload-retention-recovery");
    let state_root = temporary.path().join("state");
    let initial = coordinator(&state_root, &installation);
    let catalog_one = catalog(&installation, 1);
    let catalog_two = catalog(&installation, 2);
    let snapshot_one = snapshot(&installation, 1, 1, 'a');
    let snapshot_two = snapshot(&installation, 2, 2, 'b');
    initial.catalog_store().publish(&catalog_one).await.unwrap();
    initial.catalog_store().publish(&catalog_two).await.unwrap();
    initial
        .descriptor_snapshot_store()
        .publish(&snapshot_one)
        .await
        .unwrap();
    initial
        .descriptor_snapshot_store()
        .publish(&snapshot_two)
        .await
        .unwrap();

    let catalog_two_digest = catalog_two.descriptor_digest().unwrap();
    let snapshot_two_digest = snapshot_two.digest().unwrap();
    let plan = initial
        .plan_retention(
            std::slice::from_ref(&catalog_two_digest),
            std::slice::from_ref(&snapshot_two_digest),
        )
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    // Simulate a process stop after the first owner completed its own durable
    // journal but before the coordinator reached the second owner.
    initial
        .catalog_store()
        .apply_retention(&plan.catalog_plan, &plan.catalog_plan_digest)
        .await
        .unwrap();
    seed_test_journal(&state_root, &plan, &plan_digest, true)
        .await
        .unwrap();

    let restarted = coordinator(&state_root, &installation);
    let blocked = restarted.catalog_store().list().await.unwrap_err();
    assert_eq!(
        blocked.code,
        "use.control.capability_payload_retention_stale"
    );
    let recovered = restarted.recover_retention().await.unwrap().unwrap();
    assert!(!recovered.catalog.changed);
    assert!(recovered.descriptor_snapshot.changed);
    assert_eq!(restarted.catalog_store().list().await.unwrap().len(), 1);
    assert_eq!(
        restarted
            .descriptor_snapshot_store()
            .keys()
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(!state_root
        .join("capability-gateway/.retention-coordinator.journal")
        .exists());
}

#[tokio::test]
async fn coordinator_repairs_a_torn_phase_tail_before_recovery() {
    let temporary = tempfile::tempdir().unwrap();
    let installation = installation("capability-payload-retention-tail");
    let state_root = temporary.path().join("state");
    let coordinator = coordinator(&state_root, &installation);
    let catalog_one = catalog(&installation, 1);
    let catalog_two = catalog(&installation, 2);
    let snapshot_one = snapshot(&installation, 1, 1, 'a');
    let snapshot_two = snapshot(&installation, 2, 2, 'b');
    coordinator
        .catalog_store()
        .publish(&catalog_one)
        .await
        .unwrap();
    coordinator
        .catalog_store()
        .publish(&catalog_two)
        .await
        .unwrap();
    coordinator
        .descriptor_snapshot_store()
        .publish(&snapshot_one)
        .await
        .unwrap();
    coordinator
        .descriptor_snapshot_store()
        .publish(&snapshot_two)
        .await
        .unwrap();
    let plan = coordinator
        .plan_retention(
            std::slice::from_ref(&catalog_two.descriptor_digest().unwrap()),
            std::slice::from_ref(&snapshot_two.digest().unwrap()),
        )
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    seed_test_journal(&state_root, &plan, &plan_digest, false)
        .await
        .unwrap();
    let journal_path = state_root.join("capability-gateway/.retention-coordinator.journal");
    let mut journal = tokio::fs::OpenOptions::new()
        .append(true)
        .open(&journal_path)
        .await
        .unwrap();
    journal.write_all(br#"{"schema":"torn"}"#).await.unwrap();
    journal.sync_all().await.unwrap();

    let recovered = coordinator.recover_retention().await.unwrap().unwrap();
    assert!(recovered.catalog.changed);
    assert!(recovered.descriptor_snapshot.changed);
    assert!(!journal_path.exists());
}

#[test]
fn coordinator_rejects_mismatched_owner_roots() {
    let installation = installation("capability-payload-retention-roots");
    let first_root = tempfile::tempdir().unwrap();
    let second_root = tempfile::tempdir().unwrap();
    let catalog_store =
        CapabilityGatewayCatalogStore::new(first_root.path().join("state"), installation.clone())
            .unwrap();
    let snapshot_store = ControlCapabilityDescriptorSnapshotStore::new(
        second_root.path().join("state"),
        installation,
    )
    .unwrap();
    let error = ControlCapabilityPayloadRetentionCoordinator::new(catalog_store, snapshot_store)
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.control.capability_payload_retention_invalid"
    );
}
