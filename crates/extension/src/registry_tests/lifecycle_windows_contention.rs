use super::*;

async fn prepared_package(
    root: &Path,
    generation: u64,
) -> (
    ExtensionRegistry,
    ExtensionLifecyclePackage,
    ExtensionLifecycleIdentity,
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
    (registry(root), candidate, identity)
}

fn lifecycle_staging_paths(parent: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(parent)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(".lifecycle-staging-"))
        })
        .collect()
}

#[tokio::test]
async fn lifecycle_commit_waits_for_a_transient_scanner_lock_on_active_staging() {
    let temp = tempfile::tempdir().unwrap();
    let (registry, candidate, identity) = prepared_package(temp.path(), 14).await;
    let target = registry.lifecycle_package_root(&identity);
    let package_parent = target.parent().unwrap().to_path_buf();
    let snapshot_before_commit = registry.snapshot().await.unwrap();
    let (scanner_sender, scanner_receiver) = tokio::sync::oneshot::channel();
    crate::registry::lifecycle::install_before_candidate_commit_hook(
        target.clone(),
        Box::new(move |staging| {
            let scanner =
                crate::test_filesystem::open_directory_scanner_without_delete_share(staging);
            scanner_sender.send(scanner).unwrap();
        }),
    );
    let mut commit = Box::pin(registry.commit_lifecycle_package(&identity, &candidate));
    let scanner = tokio::select! {
        result = &mut commit => {
            panic!("lifecycle commit completed before the active staging scanner opened: {result:?}")
        }
        scanner = scanner_receiver => scanner.unwrap(),
    };

    tokio::select! {
        result = &mut commit => {
            panic!("lifecycle commit completed while the scanner denied active staging rename: {result:?}")
        }
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }

    drop(scanner);
    let committed = commit.await.unwrap();
    assert!(committed.changed);
    assert_eq!(
        committed.registry_generation,
        snapshot_before_commit.generation
    );
    assert!(target.is_dir());
    assert!(registry
        .paths()
        .receipt_path(identity.package_id())
        .is_file());
    assert!(lifecycle_staging_paths(&package_parent).is_empty());
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before_commit);
}

#[tokio::test]
async fn lifecycle_commit_bounds_a_persistent_active_staging_lock_and_replays_exactly() {
    let temp = tempfile::tempdir().unwrap();
    let (registry, candidate, identity) = prepared_package(temp.path(), 15).await;
    let target = registry.lifecycle_package_root(&identity);
    let package_parent = target.parent().unwrap().to_path_buf();
    let receipt_path = registry.paths().receipt_path(identity.package_id());
    let snapshot_before_commit = registry.snapshot().await.unwrap();
    let (scanner_sender, scanner_receiver) = tokio::sync::oneshot::channel();
    crate::registry::lifecycle::install_before_candidate_commit_hook(
        target.clone(),
        Box::new(move |staging| {
            let scanner =
                crate::test_filesystem::open_directory_scanner_without_delete_share(staging);
            scanner_sender.send(scanner).unwrap();
        }),
    );
    let started = std::time::Instant::now();
    let mut commit = Box::pin(registry.commit_lifecycle_package(&identity, &candidate));
    let scanner = tokio::select! {
        result = &mut commit => {
            panic!("lifecycle commit completed before the active staging scanner opened: {result:?}")
        }
        scanner = scanner_receiver => scanner.unwrap(),
    };

    let error = commit
        .await
        .expect_err("a persistent active staging lock must stop at the retry bound");
    let elapsed = started.elapsed();

    assert_eq!(error.code, "use.extension.io");
    assert!(elapsed >= Duration::from_secs(2));
    assert!(elapsed < Duration::from_secs(10));
    assert!(!target.exists());
    assert!(!receipt_path.exists());
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before_commit);
    let residual = lifecycle_staging_paths(&package_parent);
    assert_eq!(residual.len(), 1);
    assert!(residual[0].is_dir());

    drop(scanner);
    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    assert_eq!(
        committed.registry_generation,
        snapshot_before_commit.generation
    );
    assert!(target.is_dir());
    assert!(receipt_path.is_file());
    assert!(lifecycle_staging_paths(&package_parent).is_empty());
    assert_eq!(registry.snapshot().await.unwrap(), snapshot_before_commit);

    let replay = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(
        replay.registry_generation,
        snapshot_before_commit.generation
    );
}

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
    let snapshot_after_removal = registry.snapshot().await.unwrap();
    assert_eq!(
        snapshot_after_removal.generation,
        snapshot_before_removal.generation + 1
    );
    assert!(snapshot_after_removal.routes.is_empty());
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
    assert_eq!(
        snapshot_after_failure.generation,
        snapshot_before_removal.generation + 1
    );
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
