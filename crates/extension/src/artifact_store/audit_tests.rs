use sha2::{Digest, Sha256};

use super::*;

fn raw_digest(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn write_blob(store: &ArtifactStore, digest: &str, bytes: &[u8]) {
    let path = store.blob_path(digest).unwrap();
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

async fn write_expanded_package(
    store: &ArtifactStore,
    temporary: &tempfile::TempDir,
    manifest: &[u8],
    readme: &[u8],
) -> String {
    let source = temporary.path().join("source-package");
    std::fs::create_dir_all(source.join("nested")).unwrap();
    std::fs::write(source.join("a3s-use-extension.acl"), manifest).unwrap();
    std::fs::write(source.join("nested/README.md"), readme).unwrap();
    let fingerprint = crate::digest::package_fingerprint(&source).await.unwrap();
    let digest = format!("sha256:{}", fingerprint.sha256);
    let target = store.expanded_package_path(&digest).unwrap();
    crate::package::copy_package(&source, &target)
        .await
        .unwrap();
    digest
}

#[test]
fn digest_audit_evidence_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}

    assert_send_sync::<ArtifactStoreDigestAudit>();
    assert_send_sync::<ArtifactDigestAuditEntry>();
    assert_send_sync::<ArtifactDigestAuditStatus>();
}

#[tokio::test]
async fn digest_audit_is_deterministic_path_free_and_kind_aware() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let blob_bytes = b"blob";
    let blob_digest = raw_digest(blob_bytes);
    write_blob(&store, &blob_digest, blob_bytes);
    let package_digest = write_expanded_package(&store, &temporary, b"extension", b"readme").await;
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

    let audit = store.audit_digests(&collection).await.unwrap();

    assert_eq!(audit.schema, ARTIFACT_STORE_DIGEST_AUDIT_SCHEMA);
    assert_eq!(audit.verified_artifacts, 2);
    assert_eq!(audit.mismatched_artifacts, 0);
    assert_eq!(audit.incomplete_artifacts, 1);
    assert_eq!(audit.audited_bytes, 19);
    assert_eq!(audit.audited_files, 3);
    assert_eq!(audit.entries.len(), 3);
    assert_eq!(audit.entries[0].kind, ArtifactKind::Blob);
    assert_eq!(audit.entries[0].digest, blob_digest);
    assert_eq!(audit.entries[0].status, ArtifactDigestAuditStatus::Verified);
    assert_eq!(
        audit.entries[0].observed_digest.as_deref(),
        Some(audit.entries[0].digest.as_str())
    );
    assert_eq!(audit.entries[1].kind, ArtifactKind::Blob);
    assert_eq!(audit.entries[1].digest, incomplete_digest);
    assert_eq!(
        audit.entries[1].status,
        ArtifactDigestAuditStatus::Incomplete
    );
    assert_eq!(audit.entries[1].observed_digest, None);
    assert_eq!(audit.entries[1].staging_entries, 1);
    assert_eq!(audit.entries[1].staging_bytes, 7);
    assert_eq!(audit.entries[2].kind, ArtifactKind::ExpandedPackage);
    assert_eq!(audit.entries[2].digest, package_digest);
    assert_eq!(audit.entries[2].status, ArtifactDigestAuditStatus::Verified);

    let json = serde_json::to_value(&audit).unwrap();
    let serialized = serde_json::to_string(&json).unwrap();
    let temporary_path = temporary.path().to_string_lossy();
    assert!(!serialized.contains(temporary_path.as_ref() as &str));
    assert!(!serialized.contains("deletionAuthorized"));
    assert!(!serialized.contains("quarantineAuthorized"));
    assert!(!serialized.contains("repairAuthorized"));
    assert!(!serialized.contains("rehydrationAuthorized"));
    assert!(json["entries"][1].get("observedDigest").is_none());
}

#[tokio::test]
async fn digest_audit_reports_same_length_blob_corruption_as_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let expected = b"good";
    let corrupted = b"evil";
    let expected_digest = raw_digest(expected);
    write_blob(&store, &expected_digest, corrupted);
    let collection = store.acquire_collection().await.unwrap();

    let audit = store.audit_digests(&collection).await.unwrap();

    assert_eq!(audit.verified_artifacts, 0);
    assert_eq!(audit.mismatched_artifacts, 1);
    assert_eq!(audit.incomplete_artifacts, 0);
    assert_eq!(audit.audited_bytes, 4);
    assert_eq!(audit.entries[0].digest, expected_digest);
    assert_eq!(audit.entries[0].status, ArtifactDigestAuditStatus::Mismatch);
    assert_eq!(
        audit.entries[0].observed_digest.as_deref(),
        Some(raw_digest(corrupted).as_str())
    );
}

#[tokio::test]
async fn digest_audit_reports_same_length_expanded_package_corruption_as_evidence() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = write_expanded_package(&store, &temporary, b"extension", b"readme").await;
    let package = store.expanded_package_path(&digest).unwrap();
    std::fs::write(package.join("nested/README.md"), b"damage").unwrap();
    let collection = store.acquire_collection().await.unwrap();

    let audit = store.audit_digests(&collection).await.unwrap();

    assert_eq!(audit.verified_artifacts, 0);
    assert_eq!(audit.mismatched_artifacts, 1);
    assert_eq!(audit.entries[0].kind, ArtifactKind::ExpandedPackage);
    assert_eq!(audit.entries[0].digest, digest);
    assert_eq!(audit.entries[0].status, ArtifactDigestAuditStatus::Mismatch);
    assert_ne!(
        audit.entries[0].observed_digest.as_deref(),
        Some(digest.as_str())
    );
}

#[tokio::test]
async fn digest_audit_requires_the_exact_collection_store() {
    let temporary = tempfile::tempdir().unwrap();
    let first = ArtifactStore::from_data_root(&temporary.path().join("first"));
    let second = ArtifactStore::from_data_root(&temporary.path().join("second"));
    let collection = first.acquire_collection().await.unwrap();

    let error = second.audit_digests(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.collection_mismatch");
}

#[tokio::test]
async fn digest_audit_keeps_publication_frozen_until_the_caller_drops_the_guard() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let collection = store.acquire_collection().await.unwrap();

    let audit = store.audit_digests(&collection).await.unwrap();
    assert!(audit.entries.is_empty());
    let error = store.acquire_reference_admission().await.unwrap_err();
    assert_eq!(error.code, "use.artifact_store.busy");

    drop(collection);
    let _admission = store.acquire_reference_admission().await.unwrap();
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn digest_audit_fails_closed_on_links_in_expanded_content() {
    let temporary = tempfile::tempdir().unwrap();
    let store = ArtifactStore::from_data_root(&temporary.path().join("data"));
    let digest = format!("sha256:{}", "a".repeat(64));
    let content = store.expanded_package_path(&digest).unwrap();
    let external = temporary.path().join("external");
    std::fs::create_dir_all(&content).unwrap();
    std::fs::create_dir_all(&external).unwrap();
    std::fs::write(external.join("payload"), b"outside").unwrap();
    crate::test_filesystem::create_directory_link(&external, &content.join("linked"));
    let collection = store.acquire_collection().await.unwrap();

    let error = store.audit_digests(&collection).await.unwrap_err();

    assert_eq!(error.code, "use.artifact_store.ownership_invalid");
}
