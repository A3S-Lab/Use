use sha2::{Digest, Sha256};

use super::*;

const QUARANTINE_RECORD_NAME: &str = "quarantine.json";
const QUARANTINE_TEMPORARY_NAME: &str = ".quarantine.tmp";

fn raw_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_blob(store: &ArtifactStore, digest: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = store.blob_path(digest).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    path
}

async fn write_corrupt_package(
    store: &ArtifactStore,
    temporary: &tempfile::TempDir,
) -> (String, std::path::PathBuf) {
    let source = temporary.path().join("source-package");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("a3s-use-extension.acl"), b"extension").unwrap();
    std::fs::write(source.join("nested/README.md"), b"readme").unwrap();
    let fingerprint = crate::digest::package_fingerprint(&source).await.unwrap();
    let digest = format!("sha256:{}", fingerprint.sha256);
    let target = store.expanded_package_path(&digest).unwrap();
    crate::package::copy_package(&source, &target)
        .await
        .unwrap();
    std::fs::write(target.join("nested/README.md"), b"damage").unwrap();
    (digest, target)
}

#[test]
fn quarantine_evidence_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ArtifactQuarantinePlan>();
    assert_send_sync::<ArtifactQuarantineRecord>();
    assert_send_sync::<ArtifactQuarantineResult>();
}

#[test]
fn quarantine_plan_digest_is_canonical_and_stable() {
    let plan = ArtifactQuarantinePlan {
        schema: ARTIFACT_QUARANTINE_PLAN_SCHEMA.to_owned(),
        kind: ArtifactKind::Blob,
        digest: format!("sha256:{}", "a".repeat(64)),
        observed_digest: format!("sha256:{}", "b".repeat(64)),
        content_bytes: 4,
        content_files: 1,
    };

    assert_eq!(
        plan.descriptor_digest().unwrap(),
        "sha256:75ef3745584a8d05d702cf2a0327c18602b22ca34d22213e953153df2cd2901f"
    );
}

#[tokio::test]
async fn exact_plan_quarantine_is_path_free_idempotent_and_preserves_blob_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();

    let plan = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    let plan_digest = plan.descriptor_digest().unwrap();
    assert_eq!(plan.schema, ARTIFACT_QUARANTINE_PLAN_SCHEMA);
    assert_eq!(plan.kind, ArtifactKind::Blob);
    assert_eq!(plan.digest, digest);
    assert_eq!(plan.observed_digest, raw_digest(b"evil"));
    assert_eq!(plan.content_bytes, 4);
    assert_eq!(plan.content_files, 1);

    let applied = store
        .apply_quarantine(&collection, ArtifactKind::Blob, &digest, &plan_digest)
        .await
        .unwrap();
    assert_eq!(applied.schema, ARTIFACT_QUARANTINE_RESULT_SCHEMA);
    assert!(applied.changed);
    assert_eq!(applied.plan_digest, plan_digest);
    assert_eq!(applied.record.schema, ARTIFACT_QUARANTINE_RECORD_SCHEMA);
    assert_eq!(applied.record.plan, plan);
    assert_eq!(std::fs::read(&content).unwrap(), b"evil");

    let replay = store
        .apply_quarantine(&collection, ArtifactKind::Blob, &digest, &plan_digest)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(replay.record, applied.record);
    assert_eq!(
        store
            .inspect_quarantine(&collection, ArtifactKind::Blob, &digest)
            .await
            .unwrap(),
        Some(applied.record.clone())
    );

    let inventory = store.inspect_inventory(&collection).await.unwrap();
    assert_eq!(inventory.entries.len(), 1);
    assert_eq!(inventory.entries[0].content_bytes, 4);
    assert_eq!(inventory.entries[0].content_files, 1);
    assert_eq!(inventory.entries[0].staging_entries, 0);
    assert_eq!(inventory.entries[0].staging_bytes, 0);
    let audit = store.audit_digests(&collection).await.unwrap();
    assert_eq!(audit.entries[0].status, ArtifactDigestAuditStatus::Mismatch);
    let serialized = serde_json::to_string(&applied).unwrap();
    assert!(!serialized.contains(temporary.path().to_string_lossy().as_ref() as &str));
    assert!(!serialized.contains("timestamp"));
    assert!(!serialized.contains("deletionAuthorized"));
    assert!(!serialized.contains("repairAuthorized"));

    drop(collection);
    let admission = store.acquire_reference_admission().await.unwrap();
    let sha256 = digest.strip_prefix("sha256:").unwrap();
    let open_error = store.open_blob(&admission, sha256, 4).await.unwrap_err();
    assert_eq!(open_error.code, "use.artifact_store.quarantined");
    let observe_error = store.observe_blob(sha256, 4).await.unwrap_err();
    assert_eq!(observe_error.code, "use.artifact_store.quarantined");

    let source_path = temporary.path().join("verified-source");
    tokio::fs::write(&source_path, b"good").await.unwrap();
    let mut source = tokio::fs::File::open(&source_path).await.unwrap();
    let commit_error = store
        .commit_blob(&admission, &mut source, 4, sha256)
        .await
        .unwrap_err();
    assert_eq!(commit_error.code, "use.artifact_store.quarantined");
}

#[tokio::test]
async fn quarantine_blocks_new_expanded_package_validation_without_moving_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let (digest, content) = write_corrupt_package(&store, &temporary).await;
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_quarantine(&collection, ArtifactKind::ExpandedPackage, &digest)
        .await
        .unwrap();

    store
        .apply_quarantine(
            &collection,
            ArtifactKind::ExpandedPackage,
            &digest,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        std::fs::read(content.join("nested/README.md")).unwrap(),
        b"damage"
    );
    let audit = store.audit_digests(&collection).await.unwrap();
    assert_eq!(audit.entries[0].status, ArtifactDigestAuditStatus::Mismatch);

    drop(collection);
    let sha256 = digest.strip_prefix("sha256:").unwrap();
    let error = store
        .validate_expanded_package_path(sha256, &content)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.artifact_store.quarantined");

    let admission = store.acquire_reference_admission().await.unwrap();
    let storage = store
        .acquire_storage_admission(
            &admission,
            ArtifactStorageWrite::expanded(&digest, 15, 2).unwrap(),
        )
        .await
        .unwrap();
    let commit_error = store
        .acquire_expanded_package_mutation(&admission, &storage, sha256)
        .await
        .err()
        .expect("quarantined expanded packages must reject mutation");
    assert_eq!(commit_error.code, "use.artifact_store.quarantined");
}

#[tokio::test]
async fn quarantine_requires_a_complete_mismatched_artifact() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let verified_digest = raw_digest(b"verified");
    write_blob(&store, &verified_digest, b"verified");
    let incomplete_digest = format!("sha256:{}", "f".repeat(64));
    let incomplete = store.blob_path(&incomplete_digest).unwrap();
    std::fs::create_dir_all(incomplete.parent().unwrap()).unwrap();
    std::fs::write(
        incomplete
            .parent()
            .unwrap()
            .join(".artifact-staging-interrupted.tmp"),
        b"partial",
    )
    .unwrap();
    let collection = store.acquire_collection().await.unwrap();

    let verified = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &verified_digest)
        .await
        .unwrap_err();
    assert_eq!(verified.code, "use.artifact_store.quarantine_not_required");
    let incomplete = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &incomplete_digest)
        .await
        .unwrap_err();
    assert_eq!(
        incomplete.code,
        "use.artifact_store.quarantine_not_auditable"
    );
}

#[tokio::test]
async fn quarantine_apply_rejects_stale_review_without_writing_a_record() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    std::fs::write(&content, b"drft").unwrap();

    let error = store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quarantine_plan_mismatch");
    assert!(!content
        .parent()
        .unwrap()
        .join(QUARANTINE_RECORD_NAME)
        .exists());
}

#[tokio::test]
async fn quarantine_apply_recovers_a_bounded_unpublished_record_temporary() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let record_temporary = content.parent().unwrap().join(QUARANTINE_TEMPORARY_NAME);
    std::fs::write(&record_temporary, b"partial").unwrap();
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();

    let result = store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();

    assert!(result.changed);
    assert!(!record_temporary.exists());
    assert!(content
        .parent()
        .unwrap()
        .join(QUARANTINE_RECORD_NAME)
        .is_file());
}

#[tokio::test]
async fn malformed_quarantine_state_fails_physical_inventory_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    std::fs::write(
        content.parent().unwrap().join(QUARANTINE_RECORD_NAME),
        b"{}",
    )
    .unwrap();
    let collection = store.acquire_collection().await.unwrap();

    let error = store.inspect_inventory(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quarantine_state_invalid");
}

#[tokio::test]
async fn noncanonical_quarantine_record_fails_physical_inventory_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &digest)
        .await
        .unwrap();
    let applied = store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &digest,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    std::fs::write(
        content.parent().unwrap().join(QUARANTINE_RECORD_NAME),
        serde_json::to_vec_pretty(&applied.record).unwrap(),
    )
    .unwrap();

    let error = store.inspect_inventory(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quarantine_state_invalid");
}

#[tokio::test]
async fn quarantine_record_cannot_move_across_digest_containers() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let first_digest = raw_digest(b"first-good");
    let second_digest = raw_digest(b"second-good");
    let first_content = write_blob(&store, &first_digest, b"first-evil");
    let second_content = write_blob(&store, &second_digest, b"second-evil");
    let collection = store.acquire_collection().await.unwrap();
    let plan = store
        .plan_quarantine(&collection, ArtifactKind::Blob, &first_digest)
        .await
        .unwrap();
    store
        .apply_quarantine(
            &collection,
            ArtifactKind::Blob,
            &first_digest,
            &plan.descriptor_digest().unwrap(),
        )
        .await
        .unwrap();
    std::fs::copy(
        first_content.parent().unwrap().join(QUARANTINE_RECORD_NAME),
        second_content
            .parent()
            .unwrap()
            .join(QUARANTINE_RECORD_NAME),
    )
    .unwrap();

    let error = store.inspect_inventory(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quarantine_state_invalid");
}

#[tokio::test]
async fn oversized_quarantine_temporary_fails_physical_inventory_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    std::fs::write(
        content.parent().unwrap().join(QUARANTINE_TEMPORARY_NAME),
        vec![0_u8; 8 * 1024 + 1],
    )
    .unwrap();
    let collection = store.acquire_collection().await.unwrap();

    let error = store.inspect_inventory(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.quarantine_state_invalid");
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn linked_quarantine_record_fails_physical_inventory_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = raw_digest(b"good");
    let content = write_blob(&store, &digest, b"evil");
    let external = temporary.path().join("external");
    std::fs::create_dir_all(&external).unwrap();
    crate::test_filesystem::create_directory_link(
        &external,
        &content.parent().unwrap().join(QUARANTINE_RECORD_NAME),
    );
    let collection = store.acquire_collection().await.unwrap();

    let error = store.inspect_inventory(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.ownership_invalid");
}
