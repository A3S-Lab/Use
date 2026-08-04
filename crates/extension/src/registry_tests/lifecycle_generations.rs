use super::*;

async fn published_package(
    root: &Path,
    generation: u64,
) -> (
    ExtensionRegistry,
    ExtensionLifecyclePackage,
    ExtensionLifecycleIdentity,
    PathBuf,
) {
    let source = root.join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, generation);
    let registry = registry(root);
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    (registry, candidate, identity, source)
}

async fn staged_upgrade(
    root: &Path,
    first_generation: u64,
    next_generation: u64,
) -> (
    ExtensionRegistry,
    ExtensionLifecyclePackage,
    ExtensionLifecycleIdentity,
    ExtensionLifecycleIdentity,
) {
    let (registry, candidate, first, _) = published_package(root, first_generation).await;
    let next = lifecycle_identity(&candidate, next_generation);
    registry
        .commit_lifecycle_package(&next, &candidate)
        .await
        .unwrap();
    (registry, candidate, first, next)
}

#[tokio::test]
async fn lifecycle_candidate_source_drift_leaves_no_retained_state() {
    let temporary = tempfile::tempdir().unwrap();
    let (registry, candidate, first, source) = published_package(temporary.path(), 31).await;
    let next = lifecycle_identity(&candidate, 32);
    fs::write(source.join("README.md"), b"# Changed after review\n")
        .await
        .unwrap();

    let error = registry
        .commit_lifecycle_package(&next, &candidate)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_changed");
    assert!(fs::symlink_metadata(
        registry
            .paths()
            .retained_lifecycle_receipt_directory(first.package_id())
    )
    .await
    .is_err());
    assert!(!registry.lifecycle_package_root(&next).exists());
    assert_eq!(
        registry
            .get(first.package_id())
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation,
        Some(first.generation())
    );
    assert_eq!(
        registry.snapshot().await.unwrap().routes[0].lifecycle_generation,
        Some(first.generation())
    );
}

#[cfg(unix)]
#[tokio::test]
async fn lifecycle_retained_receipt_directory_symlink_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let (registry, _, first, _) = staged_upgrade(temporary.path(), 41, 42).await;
    let directory = registry
        .paths()
        .retained_lifecycle_receipt_directory(first.package_id());
    let owned = directory.with_file_name("cognitive-owned");
    fs::rename(&directory, &owned).await.unwrap();
    std::os::unix::fs::symlink(&owned, &directory).unwrap();

    let error = registry.get_lifecycle_generation(&first).await.unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_receipt_path_invalid");
}

#[tokio::test]
async fn lifecycle_moved_retained_receipt_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let (registry, candidate, first, _) = staged_upgrade(temporary.path(), 51, 52).await;
    let package_sha256 = candidate.package_digest().strip_prefix("sha256:").unwrap();
    let original = registry.paths().retained_lifecycle_receipt_path(
        first.package_id(),
        first.generation(),
        package_sha256,
    );
    let moved = registry.paths().retained_lifecycle_receipt_path(
        first.package_id(),
        first.generation() + 10,
        package_sha256,
    );
    fs::rename(&original, &moved).await.unwrap();

    let error = registry.get_lifecycle_generation(&first).await.unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_identity_mismatch");
}

#[tokio::test]
async fn lifecycle_tampered_retained_receipt_fails_closed() {
    let temporary = tempfile::tempdir().unwrap();
    let (registry, candidate, first, _) = staged_upgrade(temporary.path(), 61, 62).await;
    let package_sha256 = candidate.package_digest().strip_prefix("sha256:").unwrap();
    let path = registry.paths().retained_lifecycle_receipt_path(
        first.package_id(),
        first.generation(),
        package_sha256,
    );
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
    receipt["lifecycleGeneration"] = serde_json::json!(first.generation() + 1);
    fs::write(&path, serde_json::to_vec_pretty(&receipt).unwrap())
        .await
        .unwrap();

    assert!(registry.get_lifecycle_generation(&first).await.is_err());
}

#[tokio::test]
async fn lifecycle_retained_generation_count_is_bounded() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 1);
    let registry = registry(temporary.path());
    let directory = registry
        .paths()
        .retained_lifecycle_receipt_directory(identity.package_id());
    fs::create_dir_all(&directory).await.unwrap();
    let package_sha256 = candidate.package_digest().strip_prefix("sha256:").unwrap();
    for generation in 1..=33 {
        let path = directory.join(format!("{generation:020}-{package_sha256}.json"));
        fs::write(path, b"{}").await.unwrap();
    }

    let error = registry
        .get_lifecycle_generation(&identity)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_receipt_limit_exceeded");
}
