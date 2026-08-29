use sha2::{Digest, Sha256};

use super::*;
use crate::{RegistrySourceInput, RegistrySourceStore, VerifiedTargetCachePolicy};

fn input(name: &str, url: &str, root: char) -> RegistrySourceInput {
    RegistrySourceInput::new(
        name,
        url,
        root.to_string().repeat(64),
        None,
        VerifiedTargetCachePolicy::default(),
    )
}

async fn write_observation(datastore: &Path, body: &[u8]) -> String {
    let digest = format!("{:x}", Sha256::digest(body));
    let cache = datastore.join("verified-targets/sha256");
    fs::create_dir_all(&cache).await.unwrap();
    fs::write(datastore.join(".target-cache.lock"), b"")
        .await
        .unwrap();
    fs::write(
        cache.join(format!("{digest}.json")),
        format!(
            "{{\"schema\":\"a3s.use.registry-target-observation.v1\",\"targetDigest\":\"sha256:{digest}\",\"expectedBytes\":{}}}",
            body.len()
        ),
    )
    .await
    .unwrap();
    digest
}

#[test]
fn inventory_types_are_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<RegistryArtifactReference>();
    assert_send_sync::<RegistryArtifactReferenceInventory>();
}

#[tokio::test]
async fn inventory_retains_replaced_sources_and_is_deterministic_and_path_free() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let store = RegistrySourceStore::new(paths.clone());
    let added = store
        .add(input("packages", "https://one.example/", '1'))
        .await
        .unwrap();
    let first = store.resolve(None).await.unwrap();
    let datastore = first.root().datastore().to_path_buf();
    fs::create_dir_all(&datastore).await.unwrap();
    let second_digest = write_observation(&datastore, b"second").await;
    let first_digest = write_observation(&datastore, b"first").await;
    let source_identity = datastore
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();

    store
        .replace(
            &added.snapshot.revision,
            input("packages", "https://two.example/", '2'),
        )
        .await
        .unwrap();
    assert_ne!(
        datastore,
        store
            .resolve(None)
            .await
            .unwrap()
            .root()
            .datastore()
            .to_path_buf()
    );

    let collection = paths.artifact_store().acquire_collection().await.unwrap();
    let inventory = store
        .inspect_artifact_references(&collection)
        .await
        .unwrap();
    assert_eq!(
        inventory.schema,
        REGISTRY_ARTIFACT_REFERENCE_INVENTORY_SCHEMA
    );
    assert_eq!(inventory.references.len(), 2);
    let expected = [first_digest, second_digest]
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    assert_eq!(
        inventory
            .references
            .iter()
            .map(|reference| reference.digest.clone())
            .collect::<Vec<_>>(),
        expected
            .iter()
            .map(|digest| format!("sha256:{digest}"))
            .collect::<Vec<_>>()
    );
    assert!(inventory.references.iter().all(|reference| {
        reference.registry_name == "packages"
            && reference.source_identity == source_identity
            && reference.expected_bytes > 0
    }));
    let encoded = serde_json::to_string(&inventory).unwrap();
    let temporary_path = temporary.path().to_string_lossy();
    assert!(!encoded.contains(&*temporary_path));
}

#[tokio::test]
async fn inventory_rejects_a_collection_guard_from_another_store() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let first_paths = UsePaths::new(first.path().join("data"), first.path().join("state"));
    let second_paths = UsePaths::new(second.path().join("data"), second.path().join("state"));
    let collection = second_paths
        .artifact_store()
        .acquire_collection()
        .await
        .unwrap();
    let error = RegistrySourceStore::new(first_paths)
        .inspect_artifact_references(&collection)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.collection_mismatch");
}

#[tokio::test]
async fn inventory_fails_closed_on_unknown_or_incomplete_source_state() {
    let temporary = tempfile::tempdir().unwrap();
    let paths = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let store = RegistrySourceStore::new(paths.clone());
    store
        .add(input("packages", "https://registry.example/", 'a'))
        .await
        .unwrap();
    let resolved = store.resolve(None).await.unwrap();
    let datastore = resolved.root().datastore();
    fs::create_dir_all(datastore.join("verified-targets/sha256"))
        .await
        .unwrap();
    let collection = paths.artifact_store().acquire_collection().await.unwrap();

    let missing_lock = store
        .inspect_artifact_references(&collection)
        .await
        .unwrap_err();
    assert_eq!(missing_lock.code, "use.extension.io");

    fs::write(datastore.join(".target-cache.lock"), b"")
        .await
        .unwrap();
    fs::write(datastore.join("future-reference-authority"), b"unknown")
        .await
        .unwrap();
    let unknown = store
        .inspect_artifact_references(&collection)
        .await
        .unwrap_err();
    assert_eq!(
        unknown.code,
        "use.extension.registry_artifact_references_invalid"
    );
}
