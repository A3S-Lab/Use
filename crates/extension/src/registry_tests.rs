use std::fs::File;
use std::io::Write;
use std::sync::Arc;
use std::time::Duration;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use super::*;

#[path = "registry_tests/lifecycle_generations.rs"]
mod lifecycle_generations;
#[path = "registry_tests/route_recovery.rs"]
mod route_recovery;

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

async fn compatible_cognitive_package(root: &Path) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/packages/plugin-v3-cognitive/package");
    crate::package::copy_package(&fixture, root).await.unwrap();
}

async fn cognitive_package_with_dependencies(
    root: &Path,
    package_id: &str,
    route: &str,
    dependencies: &[(&str, &str)],
) {
    compatible_cognitive_package(root).await;
    let path = root.join(MANIFEST_NAME);
    let mut manifest = fs::read_to_string(&path).await.unwrap();
    manifest = manifest
        .replace(
            "extension \"acme/cognitive\"",
            &format!("extension \"{package_id}\""),
        )
        .replace(
            "route          = \"cognitive\"",
            &format!("route          = \"{route}\""),
        );
    let dependency_blocks = dependencies
        .iter()
        .map(|(dependency, requirement)| {
            format!("  dependency \"{dependency}\" {{\n    version = \"{requirement}\"\n  }}\n\n")
        })
        .collect::<String>();
    manifest = manifest.replace(
        "  repository {",
        &format!("{dependency_blocks}  repository {{"),
    );
    fs::write(path, manifest).await.unwrap();
}

async fn knowledge_package_with_dependencies(
    root: &Path,
    package_id: &str,
    route: &str,
    dependencies: &[(&str, &str)],
) {
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    crate::package::copy_package(&fixture, root).await.unwrap();
    let path = root.join(MANIFEST_NAME);
    let mut manifest = fs::read_to_string(&path).await.unwrap();
    manifest = manifest
        .replace(
            "extension \"acme/knowledge\"",
            &format!("extension \"{package_id}\""),
        )
        .replace(
            "route          = \"knowledge\"",
            &format!("route          = \"{route}\""),
        );
    let dependency_blocks = dependencies
        .iter()
        .map(|(dependency, requirement)| {
            format!("  dependency \"{dependency}\" {{\n    version = \"{requirement}\"\n  }}\n\n")
        })
        .collect::<String>();
    manifest = manifest.replace(
        "  repository {",
        &format!("{dependency_blocks}  repository {{"),
    );
    fs::write(path, manifest).await.unwrap();
}

async fn verified_knowledge_catalog(
    root: &Path,
    package_id: &str,
    dependencies: &[(&str, &str)],
    seed: char,
) -> VerifiedPluginCatalogRecord {
    let (_, manifest_bytes) = read_manifest(root).await.unwrap();
    let fingerprint = crate::digest::package_fingerprint(root).await.unwrap();
    let mut catalog = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    let (publisher, name) = package_id.split_once('/').unwrap();
    catalog.package_id = package_id.to_string();
    catalog.publisher = publisher.to_string();
    catalog.display_name = format!("{publisher} {name}");
    catalog.description = format!("Lifecycle graph fixture for {package_id}.");
    catalog.repository = format!("https://github.com/{publisher}/{name}");
    catalog.target = "any".to_string();
    catalog.dependencies = dependencies
        .iter()
        .map(|(dependency, requirement)| {
            a3s_use_core::PluginPackageDependency::new(*dependency, *requirement).unwrap()
        })
        .collect();
    catalog.archive.target_name =
        format!("extensions/{package_id}/1.0.0/stable/any/{publisher}-{name}-1.0.0-any.tar.gz");
    catalog.archive.length = 1;
    catalog.archive.sha256 = format!("sha256:{}", seed.to_string().repeat(64));
    catalog.package.expanded_bytes = fingerprint.byte_count;
    catalog.package.file_count = fingerprint.file_count;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.sha256));
    catalog.package.manifest_sha256 = Some(format!("sha256:{}", sha256(&manifest_bytes)));
    catalog.validate().unwrap();
    let provenance = a3s_use_core::VerifiedCatalogProvenance {
        registry_name: "fixture".to_string(),
        registry_url: "https://packages.example.test/catalog/".to_string(),
        root_sha256: format!("sha256:{}", "f".repeat(64)),
        root_version: 1,
        timestamp_version: 1,
        snapshot_version: 1,
        targets_version: 1,
        catalog_record_digest: catalog.descriptor_digest().unwrap(),
    };
    VerifiedPluginCatalogRecord::new(catalog, provenance).unwrap()
}

async fn bind_remote_catalog_receipt(
    registry: &ExtensionRegistry,
    package_id: &str,
    catalog: &VerifiedPluginCatalogRecord,
) {
    let mut receipt = registry.get(package_id).await.unwrap().unwrap().receipt;
    receipt.trust = ExtensionTrust::RegistryTuf;
    receipt.registry = Some(ResolvedRemotePackage::from_verified_catalog(catalog).unwrap());
    receipt.verified_catalog = Some(catalog.clone());
    write_receipt(&registry.paths().receipt_path(package_id), &receipt)
        .await
        .unwrap();
}

fn lifecycle_identity(
    candidate: &ExtensionLifecyclePackage,
    generation: u64,
) -> ExtensionLifecycleIdentity {
    ExtensionLifecycleIdentity::new(
        candidate.package_id(),
        candidate.package_digest(),
        candidate.manifest_digest(),
        generation,
    )
    .unwrap()
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
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        #[cfg(unix)]
        let options = {
            let mode = std::fs::metadata(&source_file)
                .unwrap()
                .permissions()
                .mode();
            options.unix_permissions(mode)
        };
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
async fn rejects_external_repository_packages_for_an_incompatible_host() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.2.0").await;
    let manifest_path = source.join(MANIFEST_NAME);
    let manifest = fs::read_to_string(&manifest_path)
        .await
        .unwrap()
        .replace("schema_version = 1", "schema_version = 2")
        .replace(
            "route = \"slack\"",
            concat!(
                "route = \"slack\"\n",
                "  requires_use = \">=99.0.0\"\n\n",
                "  repository {\n",
                "    url = \"https://github.com/acme/slack\"\n",
                "  }"
            ),
        );
    fs::write(&manifest_path, manifest).await.unwrap();

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

    assert_eq!(error.code, "use.extension.host_incompatible");
}

#[tokio::test]
async fn installs_a_release_bundle_only_with_the_reviewed_package_digest() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("release/a3s/science");
    package(&source, "a3s/science", "science", "1.2.0").await;
    let bundle = crate::inspect_release_bundle(&source).await.unwrap();
    let registry = registry(temp.path());

    let changed = registry
        .install_release_bundle("a3s/science", &source, &bundle.package_sha256, false)
        .await
        .unwrap();
    assert!(changed.changed);
    assert_eq!(
        changed.extension.receipt.trust,
        ExtensionTrust::ReleaseBundle
    );
    assert_eq!(
        changed.extension.receipt.package_sha256.as_deref(),
        Some(bundle.package_sha256.as_str())
    );
    assert!(changed.extension.receipt.registry.is_none());

    fs::write(source.join("skills/demo/SKILL.md"), "# Changed\n")
        .await
        .unwrap();
    let error = registry
        .install_release_bundle("a3s/science", &source, &bundle.package_sha256, true)
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.release_bundle_changed");
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
async fn receipt_v2_requires_plan_ready_catalog_evidence() {
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
    invalid["schemaVersion"] = serde_json::json!(2);
    fs::write(&receipt_path, serde_json::to_vec_pretty(&invalid).unwrap())
        .await
        .unwrap();

    let error = registry.get("acme/slack").await.unwrap_err();
    assert_eq!(error.code, "use.extension.receipt_invalid");
}

#[tokio::test]
async fn receipt_digest_is_stable_and_binds_desired_state() {
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
        .unwrap()
        .extension;

    let digest = installed.receipt.descriptor_digest().unwrap();
    assert_eq!(installed.receipt.descriptor_digest().unwrap(), digest);
    assert!(digest.starts_with("sha256:"));
    let mut disabled = installed.receipt;
    disabled.enabled = false;
    assert_ne!(disabled.descriptor_digest().unwrap(), digest);
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

#[path = "registry_tests/cognitive_lifecycle.rs"]
mod cognitive_lifecycle;
