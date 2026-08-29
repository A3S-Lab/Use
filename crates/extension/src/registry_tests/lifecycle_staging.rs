use super::*;

#[tokio::test]
async fn lifecycle_commit_reclaims_abandoned_physical_staging_directories() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 16);
    let registry = registry(temp.path());
    let target = registry.lifecycle_package_root(&identity);
    let package_parent = target.parent().unwrap();
    let abandoned = package_parent.join(".artifact-staging-abandoned");
    fs::create_dir_all(abandoned.join("nested")).await.unwrap();
    fs::write(abandoned.join("nested/partial"), b"partial package")
        .await
        .unwrap();

    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();

    assert!(committed.changed);
    assert!(!abandoned.exists());
    assert!(target.is_dir());
}

#[cfg(windows)]
#[tokio::test]
async fn lifecycle_commit_waits_for_a_transient_scanner_lock_on_abandoned_staging() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 16);
    let registry = registry(temp.path());
    let target = registry.lifecycle_package_root(&identity);
    let package_parent = target.parent().unwrap();
    let abandoned = package_parent.join(".artifact-staging-abandoned");
    let partial = abandoned.join("nested/partial");
    fs::create_dir_all(partial.parent().unwrap()).await.unwrap();
    fs::write(&partial, b"partial package").await.unwrap();
    let scanner = crate::test_filesystem::open_reading_scanner_without_delete_share(&partial);
    let mut commit = Box::pin(registry.commit_lifecycle_package(&identity, &candidate));

    tokio::select! {
        result = &mut commit => {
            panic!("lifecycle commit completed while the scanner denied staging deletion: {result:?}")
        }
        () = tokio::time::sleep(Duration::from_millis(200)) => {}
    }

    drop(scanner);
    let committed = commit.await.unwrap();
    assert!(committed.changed);
    assert!(!abandoned.exists());
    assert!(target.is_dir());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn lifecycle_commit_rejects_an_abandoned_staging_link_without_following_it() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 16);
    let registry = registry(temp.path());
    let target = registry.lifecycle_package_root(&identity);
    let package_parent = target.parent().unwrap();
    fs::create_dir_all(package_parent).await.unwrap();
    let external = temp.path().join("external");
    fs::create_dir_all(&external).await.unwrap();
    fs::write(external.join("sentinel"), b"do not remove")
        .await
        .unwrap();
    let abandoned = package_parent.join(".artifact-staging-linked");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &abandoned).unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&abandoned)
            .arg(&external)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let error = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_store.ownership_invalid");
    assert_eq!(
        fs::read(external.join("sentinel")).await.unwrap(),
        b"do not remove"
    );
    assert!(!target.exists());
    assert!(!registry.paths().receipt_path("acme/cognitive").exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn lifecycle_commit_rejects_a_package_parent_link_without_writing_outside_data_root() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 16);
    let registry = registry(temp.path());
    fs::create_dir_all(registry.paths().use_paths().data_root())
        .await
        .unwrap();
    let external = temp.path().join("external-packages");
    fs::create_dir_all(&external).await.unwrap();
    fs::write(external.join("sentinel"), b"do not modify")
        .await
        .unwrap();
    let linked_artifacts = registry.paths().artifact_store().root().to_path_buf();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &linked_artifacts).unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&linked_artifacts)
            .arg(&external)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let error = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.artifact_store.ownership_invalid");
    assert_eq!(
        fs::read(external.join("sentinel")).await.unwrap(),
        b"do not modify"
    );
    assert_eq!(std::fs::read_dir(&external).unwrap().count(), 1);
    assert!(!registry.paths().receipt_path("acme/cognitive").exists());
}

#[cfg(any(unix, windows))]
#[tokio::test]
async fn installed_artifact_rejects_an_ancestor_replaced_by_a_link() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cognitive");
    compatible_cognitive_package(&source).await;
    let candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let identity = lifecycle_identity(&candidate, 16);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();

    let artifact_root = registry.paths().artifact_store().root().to_path_buf();
    let external = temp.path().join("external-artifacts");
    std::fs::rename(&artifact_root, &external).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &artifact_root).unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&artifact_root)
            .arg(&external)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let error = registry.get("acme/cognitive").await.unwrap_err();
    assert_eq!(error.code, "use.artifact_store.ownership_invalid");
    assert!(external.join("expanded-packages").is_dir());
}
