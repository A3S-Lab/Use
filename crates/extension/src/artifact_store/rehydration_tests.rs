use sha2::{Digest, Sha256};

use super::*;

fn raw_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_blob(store: &ArtifactStore, digest: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = store.blob_path(digest).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

async fn write_package(root: &std::path::Path, body: &[u8]) -> crate::digest::PackageFingerprint {
    std::fs::create_dir_all(root.join("nested")).unwrap();
    std::fs::write(root.join("a3s-use-extension.acl"), b"extension").unwrap();
    std::fs::write(root.join("nested/README.md"), body).unwrap();
    crate::digest::package_fingerprint(root).await.unwrap()
}

#[test]
fn rehydration_evidence_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ArtifactRehydrationPlan>();
    assert_send_sync::<ArtifactRehydrationRecord>();
    assert_send_sync::<ArtifactRehydrationResult>();
}

#[test]
fn rehydration_plan_digest_is_canonical_and_stable() {
    let plan = ArtifactRehydrationPlan {
        schema: ARTIFACT_REHYDRATION_PLAN_SCHEMA.to_owned(),
        kind: ArtifactKind::Blob,
        digest: format!("sha256:{}", "a".repeat(64)),
        quarantine_plan_digest: format!("sha256:{}", "b".repeat(64)),
        quarantined_observed_digest: format!("sha256:{}", "c".repeat(64)),
        quarantined_content_bytes: 4,
        quarantined_content_files: 1,
        replacement_content_bytes: 5,
        replacement_content_files: 1,
        required_reference_count: 0,
    };

    assert_eq!(
        plan.descriptor_digest().unwrap(),
        "sha256:192bfd178a5138e4a953bb05cd3d1008ef9c97d5c84f5b6825b15910124f162a"
    );
}

#[tokio::test]
async fn exact_plan_rehydrates_an_unreferenced_blob_and_replay_is_stable() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-blob");
    std::fs::write(&candidate, b"good").unwrap();
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    let plan = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    assert_eq!(plan.schema, ARTIFACT_REHYDRATION_PLAN_SCHEMA);
    assert_eq!(plan.kind, ArtifactKind::Blob);
    assert_eq!(plan.digest, digest);
    assert_eq!(plan.quarantined_observed_digest, raw_digest(b"evil"));
    assert_eq!(plan.quarantined_content_bytes, 4);
    assert_eq!(plan.replacement_content_bytes, 4);
    assert_eq!(plan.required_reference_count, 0);
    let serialized = serde_json::to_string(&plan).unwrap();
    assert!(!serialized.contains(temporary.path().to_string_lossy().as_ref() as &str));
    assert!(!serialized.contains("candidatePath"));

    let applied = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan_digest,
        )
        .await
        .unwrap();
    assert!(applied.changed);
    assert_eq!(applied.schema, ARTIFACT_REHYDRATION_RESULT_SCHEMA);
    assert_eq!(applied.record.schema, ARTIFACT_REHYDRATION_RECORD_SCHEMA);
    assert_eq!(std::fs::read(&content).unwrap(), b"good");
    let container = content.parent().unwrap();
    assert!(container.join("quarantine.json").is_file());
    assert!(container.join("rehydration-plan.json").is_file());
    assert!(container.join("rehydration.json").is_file());
    assert!(!container.join(".artifact-staging-rehydration.tmp").exists());
    assert!(!container
        .join(".artifact-staging-rehydration-retired.tmp")
        .exists());

    std::fs::remove_file(&candidate).unwrap();
    let replay = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan_digest,
        )
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.record, applied.record);
    let audit = store.audit_digests(&collection).await.unwrap();
    assert_eq!(audit.entries[0].status, ArtifactDigestAuditStatus::Verified);

    drop(collection);
    let admission = store.acquire_reference_admission().await.unwrap();
    let mut blob = store
        .open_blob(&admission, digest.strip_prefix("sha256:").unwrap(), 4)
        .await
        .unwrap()
        .unwrap();
    let staged = temporary.path().join("staged-blob");
    blob.stage_into(&staged).await.unwrap();
    assert_eq!(std::fs::read(staged).unwrap(), b"good");
    let commit_source = temporary.path().join("commit-source");
    tokio::fs::write(&commit_source, b"good").await.unwrap();
    let mut source = tokio::fs::File::open(&commit_source).await.unwrap();
    store
        .commit_blob(
            &admission,
            &mut source,
            4,
            digest.strip_prefix("sha256:").unwrap(),
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn rehydration_rejects_a_changed_candidate_without_touching_corrupt_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-blob");
    std::fs::write(&candidate, b"good").unwrap();
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let plan = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();
    std::fs::write(&candidate, b"drft").unwrap();

    let error = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap_err();

    assert_eq!(
        error.code,
        "use.artifact_store.rehydration_candidate_mismatch"
    );
    assert_eq!(std::fs::read(content).unwrap(), b"evil");
}

#[tokio::test]
async fn rehydration_candidate_must_be_independent_of_the_artifact_store() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let candidate = content
        .parent()
        .unwrap()
        .join(".artifact-staging-candidate.tmp");
    std::fs::write(&candidate, b"good").unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    let error = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_store.rehydration_plan_invalid");
    assert_eq!(std::fs::read(content).unwrap(), b"evil");
}

#[tokio::test]
async fn rehydration_respects_peak_quota_and_resumes_from_its_prepared_record() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-blob");
    std::fs::write(&candidate, b"good").unwrap();
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let quota = store.storage_quota().await.unwrap();
    store
        .set_storage_quota(
            &quota.revision,
            ArtifactStorageQuotaPolicy::new(4, 1).unwrap(),
        )
        .await
        .unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let plan = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();

    let error = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan_digest,
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quota_exceeded");
    assert_eq!(std::fs::read(&content).unwrap(), b"evil");
    assert!(content
        .parent()
        .unwrap()
        .join("rehydration-plan.json")
        .is_file());
    assert!(!content.parent().unwrap().join("rehydration.json").exists());
    drop(collection);

    let quota = store.storage_quota().await.unwrap();
    store
        .set_storage_quota(
            &quota.revision,
            ArtifactStorageQuotaPolicy::new(8, 1).unwrap(),
        )
        .await
        .unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let recovered = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan_digest,
        )
        .await
        .unwrap();

    assert!(recovered.changed);
    assert_eq!(std::fs::read(&content).unwrap(), b"good");
}

#[tokio::test]
async fn rehydration_recovers_interrupted_preparation_without_opening_access() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-blob");
    std::fs::write(&candidate, b"good").unwrap();
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let plan = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();
    std::fs::write(
        content.parent().unwrap().join(".rehydration-plan.tmp"),
        b"partial",
    )
    .unwrap();

    let result = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert!(result.changed);
    assert_eq!(std::fs::read(&content).unwrap(), b"good");
    assert!(!content
        .parent()
        .unwrap()
        .join(".rehydration-plan.tmp")
        .exists());
}

#[tokio::test]
async fn rehydration_recovers_after_corrupt_content_was_retired() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-blob");
    std::fs::write(&candidate, b"good").unwrap();
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let container = content.parent().unwrap().to_path_buf();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let plan = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();
    let record = ArtifactRehydrationRecord {
        schema: ARTIFACT_REHYDRATION_RECORD_SCHEMA.to_owned(),
        plan_digest: plan.descriptor_digest().unwrap(),
        plan,
    };
    std::fs::write(
        container.join("rehydration-plan.json"),
        super::quarantine::canonical_json(&record).unwrap(),
    )
    .unwrap();
    std::fs::rename(
        &content,
        container.join(".artifact-staging-rehydration-retired.tmp"),
    )
    .unwrap();
    std::fs::write(container.join(".artifact-staging-rehydration.tmp"), b"good").unwrap();

    let result = store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &record.plan_digest,
        )
        .await
        .unwrap();

    assert!(result.changed);
    assert_eq!(std::fs::read(content).unwrap(), b"good");
    assert!(!container
        .join(".artifact-staging-rehydration-retired.tmp")
        .exists());
    assert!(container.join("rehydration.json").is_file());
}

#[tokio::test]
async fn rehydration_records_cannot_move_to_another_digest_container() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-blob");
    std::fs::write(&candidate, b"good").unwrap();
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let plan = store
        .plan_rehydration(&collection, ArtifactKind::Blob, &digest, &candidate)
        .await
        .unwrap();
    store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &candidate,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    let other_digest = raw_digest(b"next");
    let other_content = write_blob(&store, &other_digest, b"harm");
    for name in ["rehydration-plan.json", "rehydration.json"] {
        std::fs::copy(
            content.parent().unwrap().join(name),
            other_content.parent().unwrap().join(name),
        )
        .unwrap();
    }

    let inventory_error = store.inspect_inventory(&collection).await.unwrap_err();
    assert_eq!(
        inventory_error.code,
        "use.artifact_store.rehydration_state_invalid"
    );
    drop(collection);
    let access_error = store
        .observe_blob(other_digest.strip_prefix("sha256:").unwrap(), 4)
        .await
        .unwrap_err();
    assert_eq!(
        access_error.code,
        "use.artifact_store.rehydration_state_invalid"
    );
}

#[tokio::test]
async fn exact_plan_rehydrates_an_expanded_package() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let candidate = temporary.path().join("verified-package");
    let fingerprint = write_package(&candidate, b"readme").await;
    let digest = format!("sha256:{}", fingerprint.sha256);
    let content = store.expanded_package_path(&digest).unwrap();
    crate::package::copy_package(&candidate, &content)
        .await
        .unwrap();
    std::fs::write(content.join("nested/README.md"), b"damage").unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let quarantine = store
        .plan_quarantine(&collection, ArtifactKind::ExpandedPackage, &digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::ExpandedPackage,
            &digest,
            &quarantine.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    let plan = store
        .plan_rehydration(
            &collection,
            ArtifactKind::ExpandedPackage,
            &digest,
            &candidate,
        )
        .await
        .unwrap();

    store
        .apply_unreferenced_rehydration(
            &collection,
            ArtifactKind::ExpandedPackage,
            &digest,
            &candidate,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(
        std::fs::read(content.join("nested/README.md")).unwrap(),
        b"readme"
    );
    store
        .validate_expanded_package_path(fingerprint.sha256.as_str(), &content)
        .await
        .unwrap();

    drop(collection);
    let admission = store.acquire_reference_admission().await.unwrap();
    let storage = store
        .acquire_storage_admission(
            &admission,
            ArtifactStorageWrite::expanded(
                &digest,
                plan.replacement_content_bytes,
                plan.replacement_content_files,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let _mutation = store
        .acquire_expanded_package_mutation(
            &admission,
            &storage,
            digest.strip_prefix("sha256:").unwrap(),
        )
        .await
        .unwrap();
}
