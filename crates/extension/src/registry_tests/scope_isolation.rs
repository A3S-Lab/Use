use super::*;
use crate::UsePaths;

#[tokio::test]
async fn equal_textual_ids_in_different_kinds_select_independent_generations() {
    let temporary = tempfile::tempdir().unwrap();
    let user_source = temporary.path().join("user-source");
    let workspace_source = temporary.path().join("workspace-source");
    compatible_cognitive_package(&user_source).await;
    compatible_cognitive_package(&workspace_source).await;
    let workspace_manifest_path = workspace_source.join(MANIFEST_NAME);
    let workspace_manifest = fs::read_to_string(&workspace_manifest_path)
        .await
        .unwrap()
        .replace("version        = \"1.0.0\"", "version        = \"2.0.0\"");
    fs::write(&workspace_manifest_path, workspace_manifest)
        .await
        .unwrap();

    let user_installation =
        a3s_use_core::InstallationId::new(a3s_use_core::InstallationKind::User, "shared/research")
            .unwrap();
    let workspace_installation = a3s_use_core::InstallationId::new(
        a3s_use_core::InstallationKind::Workspace,
        "shared/research",
    )
    .unwrap();
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let user_paths = ExtensionPaths::from_roots(roots.clone(), user_installation.clone()).unwrap();
    let workspace_paths =
        ExtensionPaths::from_roots(roots, workspace_installation.clone()).unwrap();
    assert_ne!(user_paths.data_root(), workspace_paths.data_root());
    assert_ne!(user_paths.state_root(), workspace_paths.state_root());
    let user_maintenance = crate::StateMaintenanceLock::new(user_paths.state_root());
    let workspace_maintenance = crate::StateMaintenanceLock::new(workspace_paths.state_root());
    let _user_exclusive = user_maintenance.acquire_exclusive().await.unwrap();
    assert!(user_maintenance
        .try_acquire_shared()
        .await
        .unwrap()
        .is_none());
    assert!(workspace_maintenance
        .try_acquire_shared()
        .await
        .unwrap()
        .is_some());
    drop(_user_exclusive);

    let user_registry = ExtensionRegistry::new(user_paths);
    let workspace_registry = ExtensionRegistry::new(workspace_paths);
    let user_candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &user_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let workspace_candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &workspace_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let user_identity = lifecycle_identity(&user_candidate, 1);
    let workspace_identity = lifecycle_identity(&workspace_candidate, 1);

    user_registry
        .commit_lifecycle_package(&user_identity, &user_candidate)
        .await
        .unwrap();
    user_registry
        .publish_lifecycle_package(&user_identity)
        .await
        .unwrap();
    workspace_registry
        .commit_lifecycle_package(&workspace_identity, &workspace_candidate)
        .await
        .unwrap();
    workspace_registry
        .publish_lifecycle_package(&workspace_identity)
        .await
        .unwrap();

    assert_eq!(
        user_registry
            .get("acme/cognitive")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .version,
        "1.0.0"
    );
    assert_eq!(
        workspace_registry
            .get("acme/cognitive")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .version,
        "2.0.0"
    );
    let user_cursor = user_registry.snapshot().await.unwrap().cursor().unwrap();
    let workspace_cursor = workspace_registry
        .snapshot()
        .await
        .unwrap()
        .cursor()
        .unwrap();
    assert_eq!(user_cursor.installation, user_installation);
    assert_eq!(workspace_cursor.installation, workspace_installation);
    assert_eq!(
        workspace_registry
            .acquire_published_snapshot(&user_cursor)
            .await
            .unwrap_err()
            .code,
        "use.extension.snapshot_scope_mismatch"
    );

    user_registry
        .hide_lifecycle_package(&user_identity)
        .await
        .unwrap();
    let user_after = user_registry.snapshot().await.unwrap();
    assert_eq!(user_after.packages.len(), 1);
    assert!(!user_after.packages[0].enabled);
    let workspace_after = workspace_registry.snapshot().await.unwrap();
    assert_eq!(workspace_after.cursor().unwrap(), workspace_cursor);
    assert!(workspace_after.packages[0].enabled);
}

#[tokio::test]
async fn installations_share_identical_artifacts_without_sharing_lifecycle_authority() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("shared-source");
    compatible_cognitive_package(&source).await;
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let user_registry = ExtensionRegistry::new(
        roots
            .for_installation(
                a3s_use_core::InstallationId::new(
                    a3s_use_core::InstallationKind::User,
                    "shared/artifact",
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let workspace_registry = ExtensionRegistry::new(
        roots
            .for_installation(
                a3s_use_core::InstallationId::new(
                    a3s_use_core::InstallationKind::Workspace,
                    "shared/artifact",
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let user_candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let workspace_candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let user_identity = lifecycle_identity(&user_candidate, 1);
    let workspace_identity = lifecycle_identity(&workspace_candidate, 9);

    let user = user_registry
        .commit_lifecycle_package(&user_identity, &user_candidate)
        .await
        .unwrap();
    user_registry
        .publish_lifecycle_package(&user_identity)
        .await
        .unwrap();
    let workspace = workspace_registry
        .commit_lifecycle_package(&workspace_identity, &workspace_candidate)
        .await
        .unwrap();
    workspace_registry
        .publish_lifecycle_package(&workspace_identity)
        .await
        .unwrap();

    assert_eq!(
        user.extension.receipt.package_root,
        workspace.extension.receipt.package_root
    );
    assert_eq!(user.extension.receipt.lifecycle_generation, Some(1));
    assert_eq!(workspace.extension.receipt.lifecycle_generation, Some(9));

    user_registry
        .hide_lifecycle_package(&user_identity)
        .await
        .unwrap();
    user_registry
        .remove_lifecycle_package(&user_identity, std::time::Duration::from_secs(1))
        .await
        .unwrap();

    assert!(workspace.extension.receipt.package_root.is_dir());
    workspace_registry
        .get("acme/cognitive")
        .await
        .unwrap()
        .unwrap();
    assert!(workspace_registry
        .snapshot()
        .await
        .unwrap()
        .packages
        .iter()
        .any(|route| route.enabled && route.lifecycle_generation == Some(9)));
}

#[tokio::test]
async fn concurrent_installations_commit_one_complete_identical_artifact() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("shared-source");
    compatible_cognitive_package(&source).await;
    let roots = UsePaths::new(
        temporary.path().join("data"),
        temporary.path().join("state"),
    );
    let user_registry = ExtensionRegistry::new(
        roots
            .for_installation(
                a3s_use_core::InstallationId::new(
                    a3s_use_core::InstallationKind::User,
                    "concurrent/artifact",
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let workspace_registry = ExtensionRegistry::new(
        roots
            .for_installation(
                a3s_use_core::InstallationId::new(
                    a3s_use_core::InstallationKind::Workspace,
                    "concurrent/artifact",
                )
                .unwrap(),
            )
            .unwrap(),
    );
    let user_candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let workspace_candidate = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/cognitive",
        &source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let user_identity = lifecycle_identity(&user_candidate, 1);
    let workspace_identity = lifecycle_identity(&workspace_candidate, 1);

    let (user, workspace) = tokio::join!(
        user_registry.commit_lifecycle_package(&user_identity, &user_candidate),
        workspace_registry.commit_lifecycle_package(&workspace_identity, &workspace_candidate)
    );
    let user = user.unwrap();
    let workspace = workspace.unwrap();

    assert_eq!(
        user.extension.receipt.package_root,
        workspace.extension.receipt.package_root
    );
    let artifact_parent = user.extension.receipt.package_root.parent().unwrap();
    let mut entries = fs::read_dir(artifact_parent).await.unwrap();
    let mut content_directories = 0;
    while let Some(entry) = entries.next_entry().await.unwrap() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        assert!(!name.starts_with(".artifact-staging-"));
        if name == "content" {
            assert!(entry.file_type().await.unwrap().is_dir());
            content_directories += 1;
        }
    }
    assert_eq!(content_directories, 1);
}
