#![cfg(feature = "capability-catalog")]

use a3s_use::capability_catalog_store::{
    CapabilityGatewayCatalogRetentionPlan, CapabilityGatewayCatalogStore,
};
use a3s_use::core::{CapabilityGatewayCatalog, InstallationId, InstallationKind};

#[tokio::test]
async fn standalone_feature_publishes_and_recovers_an_empty_catalog() {
    let temporary = tempfile::tempdir().unwrap();
    let installation =
        InstallationId::new(InstallationKind::User, "user/catalog-store-feature").unwrap();
    let catalog = CapabilityGatewayCatalog::new(installation.clone(), 0, Vec::new()).unwrap();
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation).unwrap();

    let publication = store.publish(&catalog).await.unwrap();
    assert_eq!(store.get(&publication.digest).await.unwrap(), Some(catalog));
}

#[tokio::test]
async fn retention_is_explicit_plan_bound_and_replay_safe() {
    let temporary = tempfile::tempdir().unwrap();
    let installation =
        InstallationId::new(InstallationKind::User, "user/catalog-retention-feature").unwrap();
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let catalog = |generation| {
        CapabilityGatewayCatalog::new(installation.clone(), generation, Vec::new()).unwrap()
    };
    let first = store.publish(&catalog(0)).await.unwrap();
    let second = store.publish(&catalog(1)).await.unwrap();
    let third = store.publish(&catalog(2)).await.unwrap();

    let plan = store.plan_retention(&[third.digest.clone()]).await.unwrap();
    assert_eq!(plan.retain.len(), 1);
    assert_eq!(plan.remove.len(), 2);
    let plan_digest = plan.descriptor_digest().unwrap();
    let result = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(result.changed);
    assert_eq!(result.removed.len(), 2);
    assert_eq!(result.retained_record_count, 1);
    assert!(store.get(&first.digest).await.unwrap().is_none());
    assert!(store.get(&second.digest).await.unwrap().is_none());
    assert!(store.get(&third.digest).await.unwrap().is_some());

    // A terminal replay is read-only and does not try to delete anything a
    // second time.
    let replay = store.apply_retention(&plan, &plan_digest).await.unwrap();
    assert!(!replay.changed);
    assert!(replay.removed.is_empty());

    let empty = store.plan_retention(&[]).await.unwrap_err();
    assert_eq!(
        empty.code,
        "use.plugin.capability_gateway_catalog_retention_invalid"
    );
}

#[tokio::test]
async fn retention_rejects_stale_inventory_and_tampered_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let installation =
        InstallationId::new(InstallationKind::User, "user/catalog-retention-stale").unwrap();
    let store =
        CapabilityGatewayCatalogStore::new(temporary.path().join("state"), installation.clone())
            .unwrap();
    let first = CapabilityGatewayCatalog::new(installation.clone(), 0, Vec::new()).unwrap();
    let first_publication = store.publish(&first).await.unwrap();
    let plan = store
        .plan_retention(&[first_publication.digest.clone()])
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let second = CapabilityGatewayCatalog::new(installation, 1, Vec::new()).unwrap();
    store.publish(&second).await.unwrap();
    let stale = store
        .apply_retention(&plan, &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(
        stale.code,
        "use.plugin.capability_gateway_catalog_retention_stale"
    );

    let mut tampered = plan.clone();
    tampered.before_record_count += 1;
    let invalid = tampered.descriptor_digest().unwrap_err();
    assert_eq!(
        invalid.code,
        "use.plugin.capability_gateway_catalog_retention_invalid"
    );
    let _typed: CapabilityGatewayCatalogRetentionPlan = plan;
}
