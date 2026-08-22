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
    let abandoned = package_parent.join(".lifecycle-staging-abandoned");
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
    let abandoned = package_parent.join(".lifecycle-staging-linked");
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

    assert_eq!(error.code, "use.extension.ownership_invalid");
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
    fs::create_dir_all(registry.paths().data_root())
        .await
        .unwrap();
    let external = temp.path().join("external-packages");
    fs::create_dir_all(&external).await.unwrap();
    fs::write(external.join("sentinel"), b"do not modify")
        .await
        .unwrap();
    let linked_extensions = registry.paths().data_root().join("extensions");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&external, &linked_extensions).unwrap();
    #[cfg(windows)]
    {
        let status = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&linked_extensions)
            .arg(&external)
            .status()
            .unwrap();
        assert!(status.success());
    }

    let error = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap_err();

    assert_eq!(error.code, "use.extension.ownership_invalid");
    assert_eq!(
        fs::read(external.join("sentinel")).await.unwrap(),
        b"do not modify"
    );
    assert_eq!(std::fs::read_dir(&external).unwrap().count(), 1);
    assert!(!registry.paths().receipt_path("acme/cognitive").exists());
}
