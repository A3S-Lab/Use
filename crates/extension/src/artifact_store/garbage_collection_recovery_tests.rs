use sha2::{Digest, Sha256};

use super::*;

fn policy(digest: &str) -> ArtifactGarbageCollectionPolicy {
    ArtifactGarbageCollectionPolicy::new(vec![ArtifactGarbageCollectionTarget::new(
        ArtifactKind::ExpandedPackage,
        digest,
    )
    .unwrap()])
    .unwrap()
}

fn tombstone(container: &std::path::Path, digest: &str, plan_digest: &str) -> std::path::PathBuf {
    container.parent().unwrap().join(format!(
        ".artifact-gc-{}-{}.tmp",
        digest.strip_prefix("sha256:").unwrap(),
        plan_digest.strip_prefix("sha256:").unwrap()
    ))
}

#[tokio::test]
async fn residual_tombstone_recovery_rejects_links_without_touching_external_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = format!("sha256:{:x}", Sha256::digest(b"expanded"));
    let content = store.expanded_package_path(&digest).unwrap();
    std::fs::create_dir_all(content.join("nested")).unwrap();
    std::fs::write(content.join("nested/file"), b"bytes").unwrap();
    let policy = policy(&digest);
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    let record = ArtifactGarbageCollectionRecord {
        schema: ARTIFACT_GARBAGE_COLLECTION_RECORD_SCHEMA.to_owned(),
        plan_digest: plan_digest.clone(),
        plan,
    };
    super::garbage_collection::prepare_record_for_test(store.root(), &record)
        .await
        .unwrap();
    let container = content.parent().unwrap();
    let tombstone = tombstone(container, &digest, &plan_digest);
    std::fs::rename(container, &tombstone).unwrap();
    std::fs::remove_dir_all(tombstone.join("content")).unwrap();
    let external = temporary.path().join("external");
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("sentinel"), b"preserve").unwrap();
    crate::test_filesystem::create_directory_link(&external, &tombstone.join("content"));

    let error = store
        .apply_unreferenced_garbage_collection(&collection, policy.clone(), &plan_digest)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.artifact_store.garbage_collection_state_invalid"
    );
    assert_eq!(
        std::fs::read(external.join("sentinel")).unwrap(),
        b"preserve"
    );

    std::fs::remove_dir(tombstone.join("content")).unwrap();
    std::fs::create_dir(tombstone.join("content")).unwrap();
    store
        .apply_unreferenced_garbage_collection(&collection, policy, &plan_digest)
        .await
        .unwrap();
    assert!(!tombstone.exists());
    assert_eq!(
        std::fs::read(external.join("sentinel")).unwrap(),
        b"preserve"
    );
}

#[tokio::test]
async fn noncanonical_completion_blocks_maintenance_without_inventing_an_active_deletion() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = format!("sha256:{:x}", Sha256::digest(b"complete"));
    let content = store.expanded_package_path(&digest).unwrap();
    std::fs::create_dir_all(&content).unwrap();
    std::fs::write(content.join("file"), b"complete").unwrap();
    let policy = policy(&digest);
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    store
        .apply_unreferenced_garbage_collection(
            &collection,
            policy,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    drop(collection);
    let completion = store.root().join("garbage-collection.json");
    let mut bytes = std::fs::read(&completion).unwrap();
    bytes.push(b'\n');
    std::fs::write(&completion, bytes).unwrap();

    let admission = store.acquire_reference_admission().await.unwrap();
    drop(admission);
    let collection = store.acquire_collection().await.unwrap();
    let error = store.inspect_inventory(&collection).await.unwrap_err();
    assert_eq!(
        error.code,
        "use.artifact_store.garbage_collection_state_invalid"
    );
}
