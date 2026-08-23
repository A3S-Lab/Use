use super::*;

async fn hidden_drained_package(
    root: &Path,
    generation: u64,
) -> (ExtensionRegistry, ExtensionLifecycleIdentity) {
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
    registry.hide_lifecycle_package(&identity).await.unwrap();
    registry
        .drain_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    (registry, identity)
}

#[tokio::test]
async fn lifecycle_removal_waits_for_a_transient_scanner_lock_on_the_receipt() {
    let temp = tempfile::tempdir().unwrap();
    let (registry, identity) = hidden_drained_package(temp.path(), 12).await;
    let package_root = registry.lifecycle_package_root(&identity);
    let receipt_path = registry.paths().receipt_path(identity.package_id());
    let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&receipt_path);
    let snapshot_before_removal = registry.snapshot().await.unwrap();
    let mut removal =
        Box::pin(registry.remove_lifecycle_package(&identity, Duration::from_secs(1)));

    tokio::select! {
        result = &mut removal => {
            panic!("lifecycle removal completed while the scanner denied receipt deletion: {result:?}")
        }
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }

    drop(scanner);
    let removed = removal.await.unwrap();
    assert!(removed.changed);
    assert!(!receipt_path.exists());
    assert!(!package_root.exists());
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before_removal);
}

#[tokio::test]
async fn lifecycle_removal_bounds_a_persistent_scanner_lock_and_replays_the_residual_tree() {
    let temp = tempfile::tempdir().unwrap();
    let (registry, identity) = hidden_drained_package(temp.path(), 13).await;
    let package_root = registry.lifecycle_package_root(&identity);
    let locked_path = package_root.join(MANIFEST_NAME);
    let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&locked_path);
    let snapshot_before_removal = registry.snapshot().await.unwrap();

    let started = std::time::Instant::now();
    let error = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .expect_err("a persistent scanner lock must stop at the retry bound");
    let elapsed = started.elapsed();

    assert_eq!(error.code, "use.extension.io");
    assert!(elapsed >= Duration::from_secs(2));
    assert!(elapsed < Duration::from_secs(10));
    assert!(package_root.is_dir());
    assert!(locked_path.is_file());
    assert!(registry.get(identity.package_id()).await.unwrap().is_none());
    let snapshot_after_failure = registry.snapshot().await.unwrap();
    assert_eq!(snapshot_after_failure, snapshot_before_removal);
    assert!(snapshot_after_failure.routes.is_empty());

    drop(scanner);
    let removed = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(removed.changed);
    assert!(!package_root.exists());
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_after_failure);

    let replay = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!replay.changed);
}
