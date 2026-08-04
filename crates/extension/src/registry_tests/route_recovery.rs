use super::*;

#[tokio::test]
async fn disable_waits_for_inflight_routes_and_fails_closed_on_timeout() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let lease = registry.acquire_route("slack").await.unwrap().unwrap();

    let error = registry
        .disable_with_timeout("acme/slack", Duration::from_millis(50))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.drain_timeout");
    assert!(registry.find_route("slack").await.unwrap().is_none());
    drop(lease);

    let disabled = registry
        .disable_with_timeout("acme/slack", Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!disabled.changed);
    assert!(!disabled.enabled);
}

#[tokio::test]
async fn wait_for_change_observes_a_hot_plug_without_restarting_the_consumer() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    let initial = registry.snapshot().await.unwrap();
    assert_eq!(initial.generation, 0);

    let watcher = {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry
                .wait_for_change(initial.generation, Duration::from_secs(2))
                .await
        })
    };
    tokio::time::sleep(Duration::from_millis(50)).await;
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();

    let changed = watcher.await.unwrap().unwrap().unwrap();
    assert_eq!(changed.generation, 1);
    assert_eq!(changed.routes[0].route, "slack");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn watcher_observes_disable_while_inflight_routes_are_still_draining() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let initial = registry.snapshot().await.unwrap();
    let lease = registry.acquire_route("slack").await.unwrap().unwrap();

    let disabling = {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry
                .disable_with_timeout("acme/slack", Duration::from_secs(2))
                .await
        })
    };

    let changed = registry
        .wait_for_change(initial.generation, Duration::from_secs(1))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(changed.generation, initial.generation + 1);
    assert!(!changed.routes[0].enabled);
    drop(lease);
    disabling.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_watchers_observe_disable_without_blocking_the_lifecycle_writer() {
    const WATCHERS: usize = 32;

    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let initial = registry.snapshot().await.unwrap();
    let lease = registry.acquire_route("slack").await.unwrap().unwrap();
    let ready = Arc::new(tokio::sync::Barrier::new(WATCHERS + 1));

    let watchers = (0..WATCHERS)
        .map(|_| {
            let registry = registry.clone();
            let ready = Arc::clone(&ready);
            tokio::spawn(async move {
                ready.wait().await;
                registry
                    .wait_for_change(initial.generation, Duration::from_secs(10))
                    .await
            })
        })
        .collect::<Vec<_>>();
    ready.wait().await;

    let disabling = {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry
                .disable_with_timeout("acme/slack", Duration::from_secs(30))
                .await
        })
    };

    for watcher in watchers {
        let changed = watcher.await.unwrap().unwrap().unwrap();
        assert_eq!(changed.generation, initial.generation + 1);
        assert!(!changed.routes[0].enabled);
    }
    assert!(
        !disabling.is_finished(),
        "disable must still be draining the accepted route"
    );
    drop(lease);
    disabling.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uninstall_cannot_be_reenabled_after_visibility_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let lease = registry.acquire_route("slack").await.unwrap().unwrap();

    let uninstalling = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.uninstall("acme/slack").await })
    };
    for _ in 0..100 {
        if registry.find_route("slack").await.unwrap().is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(registry.find_route("slack").await.unwrap().is_none());
    let error = registry.enable("acme/slack").await.unwrap_err();
    assert_eq!(error.code, "use.extension.busy");

    drop(lease);
    let removed = uninstalling.await.unwrap().unwrap();
    assert!(removed.changed);
    assert!(registry.get("acme/slack").await.unwrap().is_none());
}

#[tokio::test]
async fn impossible_timeouts_are_rejected_before_lifecycle_state_changes() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();

    let error = registry
        .disable_with_timeout("acme/slack", Duration::MAX)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.timeout_invalid");
    assert!(registry.find_route("slack").await.unwrap().is_some());
    assert_eq!(registry.snapshot().await.unwrap().generation, 1);
}

#[tokio::test]
async fn snapshot_reconciles_a_receipt_commit_missed_before_publication() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();

    let mut receipt = registry.get("acme/slack").await.unwrap().unwrap().receipt;
    receipt.enabled = false;
    write_receipt(&registry.paths().receipt_path("acme/slack"), &receipt)
        .await
        .unwrap();

    let repaired = registry.snapshot().await.unwrap();
    assert_eq!(repaired.generation, 2);
    assert!(!repaired.routes[0].enabled);
    assert!(registry.find_route("slack").await.unwrap().is_none());
}

#[tokio::test]
async fn uninstall_retry_cleans_packages_after_receipt_removal_was_already_committed() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());
    let installed = registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let package_parent = registry.paths().package_parent("acme/slack");
    assert!(installed.extension.receipt.package_root.is_dir());

    fs::remove_file(registry.paths().receipt_path("acme/slack"))
        .await
        .unwrap();

    let recovered = registry.uninstall("acme/slack").await.unwrap();
    assert!(recovered.changed);
    assert!(!package_parent.exists());
    let snapshot = registry.snapshot().await.unwrap();
    assert_eq!(snapshot.generation, 2);
    assert!(snapshot.routes.is_empty());

    let unchanged = registry.uninstall("acme/slack").await.unwrap();
    assert!(!unchanged.changed);
}

#[tokio::test]
async fn lifecycle_remove_repairs_a_crash_after_hidden_receipt_deletion() {
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
    let identity = lifecycle_identity(&candidate, 23);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    registry.hide_lifecycle_package(&identity).await.unwrap();

    let hidden_snapshot = registry.snapshot().await.unwrap();
    assert_eq!(hidden_snapshot.routes.len(), 1);
    assert!(!hidden_snapshot.routes[0].enabled);
    let package_root = registry.lifecycle_package_root(&identity);
    fs::remove_file(registry.paths().receipt_path(identity.package_id()))
        .await
        .unwrap();

    let recovered = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(recovered.changed);
    assert!(!package_root.exists());
    assert!(registry.get(identity.package_id()).await.unwrap().is_none());
    assert!(registry.snapshot().await.unwrap().routes.is_empty());
}

#[tokio::test]
async fn lifecycle_remove_rejects_a_missing_receipt_while_the_generation_is_routable() {
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
    let identity = lifecycle_identity(&candidate, 29);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    let package_root = registry.lifecycle_package_root(&identity);
    fs::remove_file(registry.paths().receipt_path(identity.package_id()))
        .await
        .unwrap();

    let error = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_state_invalid");
    assert!(package_root.exists());
}
