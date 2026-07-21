use std::fs::File;
use std::io::Write;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;

const MANIFEST_NAME: &str = "a3s-use-extension.acl";

async fn package(root: &Path, package_id: &str, route: &str, version: &str) {
    fs::create_dir_all(root.join("bin")).await.unwrap();
    fs::create_dir_all(root.join("skills/demo")).await.unwrap();
    let manifest = format!(
        r#"extension "{package_id}" {{
  schema_version = 1
  version = "{version}"
  route = "{route}"
  actions = ["read"]

  cli {{
executable = "bin/extension"
json_output = true
  }}

  skill {{
path = "skills/demo/SKILL.md"
  }}

  contributes {{
    activity_bar "demo" {{
      title = "Demo"
      description = "Managed Activity Bar fixture"
      icon = "puzzle"
      entry = "web/activity.html"
      skill = "demo"
      order = 100
    }}
  }}
}}
"#
    );
    fs::write(root.join(MANIFEST_NAME), manifest).await.unwrap();
    let executable = root.join("bin/extension");
    fs::write(&executable, "#!/bin/sh\nprintf 'ok\\n'\n")
        .await
        .unwrap();
    #[cfg(unix)]
    {
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&executable, permissions).unwrap();
    }
    fs::write(root.join("skills/demo/SKILL.md"), "# Demo\n")
        .await
        .unwrap();
    fs::create_dir_all(root.join("web")).await.unwrap();
    fs::write(
        root.join("web/activity.html"),
        "<!doctype html><title>Demo</title><main>Managed activity</main>",
    )
    .await
    .unwrap();
}

fn registry(root: &Path) -> ExtensionRegistry {
    ExtensionRegistry::new(ExtensionPaths::new(root.join("data"), root.join("state")))
}

fn tar_package(source: &Path, archive: &Path) {
    let file = File::create(archive).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all("package", source).unwrap();
    builder.finish().unwrap();
}

fn zip_package(source: &Path, archive: &Path) {
    let file = File::create(archive).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    for relative in [
        "a3s-use-extension.acl",
        "bin/extension",
        "skills/demo/SKILL.md",
        "web/activity.html",
    ] {
        let source_file = source.join(relative);
        let mut options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        #[cfg(unix)]
        {
            let mode = std::fs::metadata(&source_file)
                .unwrap()
                .permissions()
                .mode();
            options = options.unix_permissions(mode);
        }
        writer
            .start_file(format!("package/{relative}"), options)
            .unwrap();
        writer
            .write_all(&std::fs::read(source_file).unwrap())
            .unwrap();
    }
    writer.finish().unwrap();
}

#[tokio::test]
async fn installs_lists_and_uninstalls_an_explicit_local_package() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.2.0").await;
    let registry = registry(temp.path());

    let result = registry
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
    assert!(result.changed);
    assert_eq!(result.extension.surfaces(), ["cli", "skill"]);
    assert!(result.extension.cli_executable().unwrap().is_file());
    assert_eq!(registry.list().await.unwrap().len(), 1);

    let unchanged = registry
        .install_local(
            "use/acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    assert!(!unchanged.changed);

    let removed = registry.uninstall("acme/slack").await.unwrap();
    assert!(removed.changed);
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn installs_and_uninstalls_a_local_tar_package() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.2.0").await;
    let archive = temp.path().join("acme-slack.tar.gz");
    tar_package(&source, &archive);
    let registry = registry(temp.path());

    let result = registry
        .install_local(
            "acme/slack",
            &archive,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.extension.receipt.package_id, "acme/slack");
    assert!(result.extension.cli_executable().unwrap().is_file());

    let removed = registry.uninstall("acme/slack").await.unwrap();
    assert!(removed.changed);
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn installs_and_uninstalls_a_local_zip_package() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.2.0").await;
    let archive = temp.path().join("acme-slack.zip");
    zip_package(&source, &archive);
    let registry = registry(temp.path());

    let result = registry
        .install_local(
            "acme/slack",
            &archive,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.extension.receipt.package_id, "acme/slack");
    assert!(result.extension.cli_executable().unwrap().is_file());

    assert!(registry.uninstall("acme/slack").await.unwrap().changed);
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_route_conflicts_and_untrusted_installs() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    package(&first, "acme/slack", "chat", "1.0.0").await;
    package(&second, "example/teams", "chat", "1.0.0").await;
    let registry = registry(temp.path());

    let error = registry
        .install_local("acme/slack", &first, InstallOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.trust_required");

    registry
        .install_local(
            "acme/slack",
            &first,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let error = registry
        .install_local(
            "example/teams",
            &second,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.route_conflict");
}

#[tokio::test]
#[cfg(unix)]
async fn rejects_package_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    std::os::unix::fs::symlink("/etc/passwd", source.join("escape")).unwrap();
    let error = registry(temp.path())
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_symlink");
}

#[tokio::test]
async fn hot_plug_disable_and_enable_publish_new_registry_generations() {
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
    let installed = registry.snapshot().await.unwrap();
    assert_eq!(installed.generation, 1);
    assert_eq!(installed.routes.len(), 1);
    assert!(installed.routes[0].enabled);
    assert!(registry.find_route("slack").await.unwrap().is_some());

    let disabled = registry
        .disable_with_timeout("acme/slack", Duration::from_secs(1))
        .await
        .unwrap();
    assert!(disabled.changed);
    assert!(!disabled.enabled);
    assert_eq!(disabled.generation, 2);
    assert!(registry.find_route("slack").await.unwrap().is_none());
    assert_eq!(registry.list().await.unwrap().len(), 1);

    let enabled = registry.enable("acme/slack").await.unwrap();
    assert!(enabled.changed);
    assert!(enabled.enabled);
    assert_eq!(enabled.generation, 3);
    assert!(registry.find_route("slack").await.unwrap().is_some());
}

#[tokio::test]
async fn hot_upgrade_keeps_the_previous_package_until_inflight_routes_drain() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    package(&first, "acme/slack", "slack", "1.0.0").await;
    package(&second, "acme/slack", "slack", "2.0.0").await;
    let second_archive = temp.path().join("second.tar.gz");
    tar_package(&second, &second_archive);
    let registry = registry(temp.path());

    let first_install = registry
        .install_local(
            "acme/slack",
            &first,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let previous_root = first_install.extension.receipt.package_root;
    let lease = registry.acquire_route("slack").await.unwrap().unwrap();

    let second_install = registry
        .install_local(
            "acme/slack",
            &second_archive,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    assert_ne!(second_install.extension.receipt.package_root, previous_root);
    assert!(previous_root.is_dir());
    assert_eq!(lease.extension().receipt.version, "1.0.0");
    assert_eq!(registry.snapshot().await.unwrap().generation, 2);
    drop(lease);
}

#[tokio::test]
async fn forced_reactivation_of_identical_metadata_publishes_a_new_generation() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());

    let first = registry
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
    let first_snapshot = registry.snapshot().await.unwrap();
    assert_eq!(first_snapshot.generation, 1);
    assert_eq!(
        first_snapshot.routes[0].package_root,
        first.extension.receipt.package_root
    );

    let second = registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: true,
            },
        )
        .await
        .unwrap();
    assert_ne!(
        second.extension.receipt.package_root,
        first.extension.receipt.package_root
    );
    assert_eq!(
        second.extension.receipt.package_sha256,
        first.extension.receipt.package_sha256
    );
    assert!(second
        .extension
        .receipt
        .package_sha256
        .as_deref()
        .is_some_and(|digest| digest.len() == 64));
    let second_snapshot = registry.snapshot().await.unwrap();
    assert_eq!(second_snapshot.generation, 2);
    assert_eq!(
        second_snapshot.routes[0].package_root,
        second.extension.receipt.package_root
    );
}

#[tokio::test]
async fn same_version_changed_executable_requires_force_and_changes_package_digest() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    let registry = registry(temp.path());

    let first = registry
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
    fs::write(
        source.join("bin/extension"),
        "#!/bin/sh\nprintf 'changed\\n'\n",
    )
    .await
    .unwrap();

    let error = registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.version_conflict");

    let second = registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: true,
            },
        )
        .await
        .unwrap();
    assert_ne!(
        second.extension.receipt.package_root,
        first.extension.receipt.package_root
    );
    assert_ne!(
        second.extension.receipt.package_sha256,
        first.extension.receipt.package_sha256
    );
    assert!(second.extension.receipt.package_sha256.is_some());
    assert_eq!(
        fs::read_to_string(second.extension.cli_executable().unwrap())
            .await
            .unwrap(),
        "#!/bin/sh\nprintf 'changed\\n'\n"
    );
}

#[tokio::test]
async fn legacy_receipt_without_package_digest_remains_readable_and_idempotent() {
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

    let receipt_path = registry.paths().receipt_path("acme/slack");
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).await.unwrap()).unwrap();
    legacy.as_object_mut().unwrap().remove("packageSha256");
    fs::write(&receipt_path, serde_json::to_vec_pretty(&legacy).unwrap())
        .await
        .unwrap();

    let installed = registry.get("acme/slack").await.unwrap().unwrap();
    assert_eq!(installed.receipt.package_sha256, None);

    let unchanged = registry
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
    assert!(!unchanged.changed);
    assert_eq!(unchanged.extension.receipt.package_sha256, None);
}

#[tokio::test]
async fn receipt_rejects_an_invalid_optional_package_digest() {
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

    let receipt_path = registry.paths().receipt_path("acme/slack");
    let mut invalid: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).await.unwrap()).unwrap();
    invalid["packageSha256"] = serde_json::json!("not-a-sha256");
    fs::write(&receipt_path, serde_json::to_vec_pretty(&invalid).unwrap())
        .await
        .unwrap();

    let error = registry.get("acme/slack").await.unwrap_err();
    assert_eq!(error.code, "use.extension.receipt_invalid");
}

#[tokio::test]
async fn snapshot_reconciles_a_pre_activation_identity_binding() {
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

    let path = registry.paths().registry_snapshot_path();
    let mut legacy: serde_json::Value =
        serde_json::from_slice(&fs::read(&path).await.unwrap()).unwrap();
    legacy["routes"][0]
        .as_object_mut()
        .unwrap()
        .remove("packageRoot");
    fs::write(&path, serde_json::to_vec_pretty(&legacy).unwrap())
        .await
        .unwrap();

    let reconciled = registry.snapshot().await.unwrap();
    assert_eq!(reconciled.generation, 2);
    assert!(!reconciled.routes[0].package_root.as_os_str().is_empty());
}

#[tokio::test]
async fn stale_route_lookup_cannot_dispatch_an_extension_after_its_route_changes() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    package(&first, "acme/slack", "slack", "1.0.0").await;
    package(&second, "acme/slack", "chat", "2.0.0").await;
    let registry = registry(temp.path());

    registry
        .install_local(
            "acme/slack",
            &first,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let stale = registry.find_route("slack").await.unwrap().unwrap();

    registry
        .install_local(
            "acme/slack",
            &second,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();

    assert!(registry
        .acquire_extension_lease(stale, Some("slack"))
        .await
        .unwrap()
        .is_none());
    assert!(registry.acquire_route("slack").await.unwrap().is_none());
    let current = registry.acquire_route("chat").await.unwrap().unwrap();
    assert_eq!(current.extension().receipt.version, "2.0.0");
}

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

    let watchers = (0..WATCHERS)
        .map(|_| {
            let registry = registry.clone();
            tokio::spawn(async move {
                registry
                    .wait_for_change(initial.generation, Duration::from_secs(2))
                    .await
            })
        })
        .collect::<Vec<_>>();
    tokio::time::sleep(Duration::from_millis(100)).await;

    let disabling = {
        let registry = registry.clone();
        tokio::spawn(async move {
            registry
                .disable_with_timeout("acme/slack", Duration::from_secs(2))
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

    // Model a process crash after the authoritative receipt replacement but
    // before the derived registry snapshot was published.
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
