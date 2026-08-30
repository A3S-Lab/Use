use sha2::{Digest, Sha256};

use super::*;

fn digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_blob(store: &ArtifactStore, bytes: &[u8]) -> (String, std::path::PathBuf) {
    let digest = digest(bytes);
    let path = store.blob_path(&digest).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    (digest, path)
}

fn policy(kind: ArtifactKind, digest: &str) -> ArtifactGarbageCollectionPolicy {
    ArtifactGarbageCollectionPolicy::new(vec![
        ArtifactGarbageCollectionTarget::new(kind, digest).unwrap()
    ])
    .unwrap()
}

#[test]
fn garbage_collection_evidence_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ArtifactGarbageCollectionTarget>();
    assert_send_sync::<ArtifactGarbageCollectionPolicy>();
    assert_send_sync::<ArtifactGarbageCollectionLifecycle>();
    assert_send_sync::<ArtifactGarbageCollectionEntry>();
    assert_send_sync::<ArtifactGarbageCollectionPlan>();
    assert_send_sync::<ArtifactGarbageCollectionRecord>();
    assert_send_sync::<ArtifactGarbageCollectionResult>();
}

#[test]
fn garbage_collection_policy_requires_explicit_unique_targets() {
    assert_eq!(
        ArtifactGarbageCollectionPolicy::new(Vec::new())
            .unwrap_err()
            .code,
        "use.artifact_store.garbage_collection_policy_invalid"
    );

    let target = ArtifactGarbageCollectionTarget::new(
        ArtifactKind::Blob,
        &format!("sha256:{}", "a".repeat(64)),
    )
    .unwrap();
    assert_eq!(
        ArtifactGarbageCollectionPolicy::new(vec![target.clone(), target])
            .unwrap_err()
            .code,
        "use.artifact_store.garbage_collection_policy_invalid"
    );
}

#[test]
fn garbage_collection_plan_digest_is_canonical_and_stable() {
    let digest = format!("sha256:{}", "a".repeat(64));
    let policy = policy(ArtifactKind::Blob, &digest);
    let plan = ArtifactGarbageCollectionPlan {
        schema: ARTIFACT_GARBAGE_COLLECTION_PLAN_SCHEMA.to_owned(),
        policy,
        predecessor_plan_digest: None,
        artifacts: vec![ArtifactGarbageCollectionEntry {
            kind: ArtifactKind::Blob,
            digest,
            physical_state: ArtifactPhysicalState::Complete,
            content_bytes: 4,
            content_files: 1,
            staging_entries: 0,
            staging_bytes: 0,
            lifecycle: ArtifactGarbageCollectionLifecycle::Ordinary,
        }],
        artifact_count: 1,
        reclaimable_bytes: 4,
        required_reference_count: 0,
    };

    assert_eq!(
        plan.descriptor_digest().unwrap(),
        "sha256:1b52fb2beb2c6c9459e2a82dd4a5f5ad1b906427cb055379472709f5df115bd1"
    );
}

#[tokio::test]
async fn exact_plan_removes_only_selected_unreferenced_artifacts_and_replays_read_only() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (removed_digest, removed_path) = write_blob(&store, b"remove");
    let (_, retained_path) = write_blob(&store, b"retain");
    let policy = policy(ArtifactKind::Blob, &removed_digest);
    let collection = store.acquire_collection().await.unwrap();

    let plan = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    assert_eq!(plan.artifacts.len(), 1);
    assert_eq!(plan.artifacts[0].digest, removed_digest);
    assert_eq!(
        plan.artifacts[0].lifecycle,
        ArtifactGarbageCollectionLifecycle::Ordinary
    );
    assert_eq!(plan.reclaimable_bytes, 6);
    let encoded = serde_json::to_string(&plan).unwrap();
    assert!(!encoded.contains(temporary.path().to_string_lossy().as_ref() as &str));

    let result = store
        .apply_unreferenced_garbage_collection(&collection, policy.clone(), &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.removed, plan.artifacts);
    assert!(!removed_path.exists());
    assert!(retained_path.exists());
    let inventory = store.inspect_inventory(&collection).await.unwrap();
    assert_eq!(inventory.entries.len(), 1);

    let replay = store
        .apply_unreferenced_garbage_collection(&collection, policy, &plan_digest)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.record, result.record);
    assert!(retained_path.exists());
}

#[tokio::test]
async fn plan_binds_stable_quarantine_lifecycle_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let expected = digest(b"good");
    let path = store.blob_path(&expected).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"evil").unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &expected)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &expected,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    let plan = store
        .plan_physical_garbage_collection(&collection, policy(ArtifactKind::Blob, &expected))
        .await
        .unwrap();

    assert_eq!(
        plan.artifacts[0].lifecycle,
        ArtifactGarbageCollectionLifecycle::Quarantined {
            quarantine_plan_digest: quarantine.descriptor_digest().unwrap(),
        }
    );
}

#[tokio::test]
async fn plan_binds_completed_rehydration_and_rejects_interrupted_recovery() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("candidate");
    std::fs::write(&candidate, b"good").unwrap();
    let expected = digest(b"good");
    let path = store.blob_path(&expected).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"evil").unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &expected)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &expected,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let rehydration = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &expected, &candidate)
        .await
        .unwrap();
    store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &expected,
            &candidate,
            &rehydration.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    let plan = store
        .plan_physical_garbage_collection(&collection, policy(ArtifactKind::Blob, &expected))
        .await
        .unwrap();
    assert_eq!(
        plan.artifacts[0].lifecycle,
        ArtifactGarbageCollectionLifecycle::Rehydrated {
            quarantine_plan_digest: quarantine.descriptor_digest().unwrap(),
            rehydration_plan_digest: rehydration.descriptor_digest().unwrap(),
        }
    );

    let interrupted_digest = digest(b"next-good");
    let interrupted_path = store.blob_path(&interrupted_digest).unwrap();
    std::fs::create_dir_all(interrupted_path.parent().unwrap()).unwrap();
    std::fs::write(&interrupted_path, b"next-evil").unwrap();
    let interrupted_quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &interrupted_digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &interrupted_digest,
            &interrupted_quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    std::fs::write(
        interrupted_path
            .parent()
            .unwrap()
            .join(".rehydration-plan.tmp"),
        b"",
    )
    .unwrap();

    let error = store
        .plan_physical_garbage_collection(
            &collection,
            policy(ArtifactKind::Blob, &interrupted_digest),
        )
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.artifact_store.garbage_collection_state_invalid"
    );
}

#[tokio::test]
async fn incomplete_staging_requires_confirmation_and_can_be_collected() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = format!("sha256:{}", "e".repeat(64));
    let content = store.blob_path(&digest).unwrap();
    let container = content.parent().unwrap();
    std::fs::create_dir_all(container).unwrap();
    std::fs::write(container.join(".artifact-staging-interrupted"), b"partial").unwrap();
    let policy = policy(ArtifactKind::Blob, &digest);
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    assert_eq!(
        plan.artifacts[0].physical_state,
        ArtifactPhysicalState::Incomplete
    );
    assert_eq!(plan.artifacts[0].staging_bytes, 7);

    store
        .apply_unreferenced_garbage_collection(
            &collection,
            policy,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    assert!(!container.exists());
}

#[tokio::test]
async fn changed_physical_evidence_invalidates_confirmation_before_deletion() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (digest, path) = write_blob(&store, b"first");
    let policy = policy(ArtifactKind::Blob, &digest);
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    std::fs::write(&path, b"changed").unwrap();

    let error = store
        .apply_unreferenced_garbage_collection(
            &collection,
            policy,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.artifact_store.garbage_collection_plan_mismatch"
    );
    assert!(path.exists());
}

#[tokio::test]
async fn interrupted_atomic_retirement_blocks_references_and_resumes_the_original_plan() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (digest, path) = write_blob(&store, b"resume");
    let policy = policy(ArtifactKind::Blob, &digest);
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
    let container = path.parent().unwrap();
    let sha256 = digest.strip_prefix("sha256:").unwrap();
    let plan_sha256 = plan_digest.strip_prefix("sha256:").unwrap();
    let tombstone = container
        .parent()
        .unwrap()
        .join(format!(".artifact-gc-{sha256}-{plan_sha256}.tmp"));
    std::fs::rename(container, &tombstone).unwrap();
    drop(collection);

    let error = store.acquire_reference_admission().await.unwrap_err();
    assert_eq!(
        error.code,
        "use.artifact_store.garbage_collection_in_progress"
    );

    let collection = store.acquire_collection().await.unwrap();
    let result = store
        .apply_unreferenced_garbage_collection(&collection, policy, &plan_digest)
        .await
        .unwrap();
    assert!(result.changed);
    assert!(!path.exists());
    assert!(!tombstone.exists());
    drop(collection);
    let _admission = store.acquire_reference_admission().await.unwrap();
}

#[tokio::test]
async fn predecessor_chaining_distinguishes_a_recreated_identical_object() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (digest, path) = write_blob(&store, b"same");
    let policy = policy(ArtifactKind::Blob, &digest);
    let collection = store.acquire_collection().await.unwrap();
    let first = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    let first_digest = first.descriptor_digest().unwrap();
    store
        .apply_unreferenced_garbage_collection(&collection, policy.clone(), &first_digest)
        .await
        .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"same").unwrap();

    let second = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    let second_digest = second.descriptor_digest().unwrap();
    assert_eq!(
        second.predecessor_plan_digest.as_deref(),
        Some(first_digest.as_str())
    );
    assert_ne!(second_digest, first_digest);

    store
        .apply_unreferenced_garbage_collection(&collection, policy, &second_digest)
        .await
        .unwrap();
    assert!(!path.exists());
}

#[tokio::test]
async fn one_plan_collects_blob_and_expanded_package_tiers() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (blob_digest, blob_path) = write_blob(&store, b"blob");
    let package_digest = format!("sha256:{}", "f".repeat(64));
    let package_path = store.expanded_package_path(&package_digest).unwrap();
    std::fs::create_dir_all(package_path.join("nested")).unwrap();
    std::fs::write(package_path.join("nested/file"), b"package").unwrap();
    let policy = ArtifactGarbageCollectionPolicy::new(vec![
        ArtifactGarbageCollectionTarget::new(ArtifactKind::ExpandedPackage, &package_digest)
            .unwrap(),
        ArtifactGarbageCollectionTarget::new(ArtifactKind::Blob, &blob_digest).unwrap(),
    ])
    .unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    assert_eq!(plan.artifacts.len(), 2);

    store
        .apply_unreferenced_garbage_collection(
            &collection,
            policy,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert!(!blob_path.exists());
    assert!(!package_path.exists());
}

#[tokio::test]
async fn a_new_plan_normalizes_terminal_state_left_before_active_cleanup() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (digest, path) = write_blob(&store, b"terminal");
    let policy = policy(ArtifactKind::Blob, &digest);
    let collection = store.acquire_collection().await.unwrap();
    let first = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    store
        .apply_unreferenced_garbage_collection(
            &collection,
            policy.clone(),
            &first.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    std::fs::copy(
        store.root().join("garbage-collection.json"),
        store.root().join("garbage-collection-plan.json"),
    )
    .unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"terminal").unwrap();

    let second = store
        .plan_physical_garbage_collection(&collection, policy.clone())
        .await
        .unwrap();
    store
        .apply_unreferenced_garbage_collection(
            &collection,
            policy,
            &second.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert!(!path.exists());
    assert!(!store.root().join("garbage-collection-plan.json").exists());
}
