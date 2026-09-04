#![cfg(feature = "capability-catalog")]

use a3s_use::capability_catalog_store::CapabilityGatewayCatalogStore;
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
