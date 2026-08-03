use std::fs::File;
use std::io::Write;
use std::sync::Arc;
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

#[tokio::test]
async fn lifecycle_commit_keeps_all_five_surfaces_installed_disabled_until_atomic_publish() {
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
    let identity = lifecycle_identity(&candidate, 7);
    let registry = registry(temp.path());

    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    assert_eq!(committed.extension.receipt.schema_version, 3);
    assert_eq!(committed.extension.receipt.lifecycle_generation, Some(7));
    assert!(!committed.extension.receipt.enabled);
    assert_eq!(
        committed.extension.surfaces(),
        ["tool", "mcp", "okf", "skill", "ui"]
    );
    assert_eq!(
        committed.extension.receipt.package_root,
        registry.lifecycle_package_root(&identity)
    );

    let installed_disabled = registry.snapshot().await.unwrap();
    assert_eq!(installed_disabled.routes.len(), 1);
    assert_eq!(installed_disabled.routes[0].lifecycle_generation, Some(7));
    assert!(!installed_disabled.routes[0].enabled);
    assert!(registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_none());

    let commit_replay = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(!commit_replay.changed);
    assert_eq!(
        commit_replay.extension.receipt.descriptor_digest().unwrap(),
        committed.extension.receipt.descriptor_digest().unwrap()
    );

    for error in [
        registry.enable("acme/cognitive").await.unwrap_err(),
        registry.disable("acme/cognitive").await.unwrap_err(),
        registry.uninstall("acme/cognitive").await.unwrap_err(),
    ] {
        assert_eq!(error.code, "use.extension.lifecycle_managed");
    }

    let published = registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    assert!(published.changed);
    assert!(published.extension.receipt.enabled);
    assert_eq!(published.extension.receipt.lifecycle_generation, Some(7));
    assert!(registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_some());

    let replay = registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(
        replay.extension.receipt.descriptor_digest().unwrap(),
        published.extension.receipt.descriptor_digest().unwrap()
    );
}

#[tokio::test]
async fn lifecycle_graph_publication_is_one_cutover_and_recovers_partial_receipts() {
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base");
    let root_source = temp.path().join("root");
    cognitive_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    cognitive_package_with_dependencies(
        &root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    let base = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/base",
        &base_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let base_identity = lifecycle_identity(&base, 31);
    let root_identity = lifecycle_identity(&root, 32);
    let identities = [base_identity.clone(), root_identity.clone()];
    let registry = registry(temp.path());
    for (identity, candidate) in [(&base_identity, &base), (&root_identity, &root)] {
        registry
            .commit_lifecycle_package(identity, candidate)
            .await
            .unwrap();
    }
    let before = registry.snapshot().await.unwrap();
    assert!(before.routes.iter().all(|route| !route.enabled));

    // Model a process crash after one receipt was enabled but before the
    // complete dependency closure reached the snapshot commit point.
    let mut partial = registry.get("acme/base").await.unwrap().unwrap().receipt;
    partial.enabled = true;
    write_receipt(&registry.paths().receipt_path("acme/base"), &partial)
        .await
        .unwrap();
    let guarded = registry.snapshot().await.unwrap();
    assert_eq!(guarded, before);
    assert!(registry
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_none());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("root", "0.3.0")
        .await
        .unwrap()
        .is_none());

    let published = registry
        .publish_lifecycle_packages_for_test_host_version(&identities, "0.3.0")
        .await
        .unwrap();
    assert_eq!(published.len(), 2);
    assert!(published.iter().all(|result| result.extension.enabled()));
    assert!(published
        .iter()
        .all(|result| result.registry_generation == before.generation + 1));
    let after = registry.snapshot().await.unwrap();
    assert_eq!(after.generation, before.generation + 1);
    assert!(after.routes.iter().all(|route| route.enabled));
    assert!(registry
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("root", "0.3.0")
        .await
        .unwrap()
        .is_some());

    let replay = registry
        .publish_lifecycle_packages_for_test_host_version(&identities, "0.3.0")
        .await
        .unwrap();
    assert!(replay.iter().all(|result| !result.changed));
    assert!(replay
        .iter()
        .all(|result| result.registry_generation == after.generation));
}

#[tokio::test]
async fn lifecycle_graph_requires_the_exact_published_retained_dependency() {
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base");
    let root_source = temp.path().join("root");
    knowledge_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    knowledge_package_with_dependencies(
        &root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    let base_catalog = verified_knowledge_catalog(&base_source, "acme/base", &[], 'a').await;
    let root_catalog =
        verified_knowledge_catalog(&root_source, "acme/root", &[("acme/base", "^1.0.0")], 'b')
            .await;
    let package_lock = a3s_use_core::PluginPackageResolver::new(
        a3s_use_core::PluginPackageLockHost::new("linux-x86_64", "0.3.0").unwrap(),
    )
    .resolve(root_catalog.clone(), vec![base_catalog.clone()])
    .unwrap();
    let base = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/base",
        &base_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let base_identity = lifecycle_identity(&base, 41);
    let root_identity = lifecycle_identity(&root, 42);
    let registry = registry(temp.path());

    registry
        .commit_lifecycle_package(&base_identity, &base)
        .await
        .unwrap();
    bind_remote_catalog_receipt(&registry, "acme/base", &base_catalog).await;
    registry
        .publish_lifecycle_package_for_host_version(&base_identity, "0.3.0")
        .await
        .unwrap();
    registry
        .commit_lifecycle_package(&root_identity, &root)
        .await
        .unwrap();
    bind_remote_catalog_receipt(&registry, "acme/root", &root_catalog).await;

    registry
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap();
    let error = registry
        .publish_lifecycle_package_graph_for_test_host_version(
            &package_lock,
            std::slice::from_ref(&root_identity),
            "0.3.0",
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.lifecycle_package_graph_invalid");
    assert!(!registry.get("acme/root").await.unwrap().unwrap().enabled());

    registry
        .publish_lifecycle_package_for_host_version(&base_identity, "0.3.0")
        .await
        .unwrap();
    let published = registry
        .publish_lifecycle_package_graph_for_test_host_version(
            &package_lock,
            std::slice::from_ref(&root_identity),
            "0.3.0",
        )
        .await
        .unwrap();
    assert_eq!(published.len(), 1);
    assert!(published[0].extension.enabled());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("base", "0.3.0")
        .await
        .unwrap()
        .is_some());
    assert!(registry
        .acquire_lifecycle_route_for_host_version("root", "0.3.0")
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn lifecycle_hide_drains_accepted_calls_before_exact_idempotent_removal() {
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
    let identity = lifecycle_identity(&candidate, 11);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    registry
        .publish_lifecycle_package_for_host_version(&identity, "0.3.0")
        .await
        .unwrap();
    let lease = registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .unwrap();

    let hidden = registry.hide_lifecycle_package(&identity).await.unwrap();
    assert!(hidden.changed);
    assert!(!hidden.extension.receipt.enabled);
    assert!(registry
        .acquire_lifecycle_route_for_host_version("cognitive", "0.3.0")
        .await
        .unwrap()
        .is_none());
    let hide_replay = registry.hide_lifecycle_package(&identity).await.unwrap();
    assert!(!hide_replay.changed);

    let error = registry
        .drain_lifecycle_package(&identity, Duration::from_millis(50))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.drain_timeout");
    drop(lease);

    let drained = registry
        .drain_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!drained.extension.receipt.enabled);
    let drain_replay = registry
        .drain_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(
        drain_replay.extension.receipt.descriptor_digest().unwrap(),
        drained.extension.receipt.descriptor_digest().unwrap()
    );
    let package_root = drained.extension.receipt.package_root.clone();

    let removed = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(removed.changed);
    assert!(!package_root.exists());
    assert!(registry.get("acme/cognitive").await.unwrap().is_none());
    assert!(registry.snapshot().await.unwrap().routes.is_empty());

    let replay = registry
        .remove_lifecycle_package(&identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(!replay.changed);
}

#[tokio::test]
async fn lifecycle_uninstall_rejects_a_dependency_until_dependents_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let base_source = temp.path().join("base");
    let root_source = temp.path().join("root");
    cognitive_package_with_dependencies(&base_source, "acme/base", "base", &[]).await;
    cognitive_package_with_dependencies(
        &root_source,
        "acme/root",
        "root",
        &[("acme/base", "^1.0.0")],
    )
    .await;
    let base = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/base",
        &base_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let root = ExtensionLifecyclePackage::prepare_local_for_host_version(
        "acme/root",
        &root_source,
        true,
        "0.3.0",
    )
    .await
    .unwrap();
    let base_identity = lifecycle_identity(&base, 21);
    let root_identity = lifecycle_identity(&root, 22);
    let registry = registry(temp.path());
    for (identity, candidate) in [(&base_identity, &base), (&root_identity, &root)] {
        registry
            .commit_lifecycle_package(identity, candidate)
            .await
            .unwrap();
        registry
            .publish_lifecycle_package_for_host_version(identity, "0.3.0")
            .await
            .unwrap();
    }

    assert_eq!(
        registry.dependent_packages("acme/base").await.unwrap(),
        ["acme/root"]
    );
    registry
        .hide_lifecycle_package(&base_identity)
        .await
        .unwrap();
    let error = registry
        .remove_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_required");
    assert_eq!(
        error.details["requiredBy"],
        serde_json::json!(["acme/root"])
    );
    assert!(registry.get("acme/base").await.unwrap().is_some());

    registry
        .hide_lifecycle_package(&root_identity)
        .await
        .unwrap();
    registry
        .remove_lifecycle_package(&root_identity, Duration::from_secs(1))
        .await
        .unwrap();
    registry
        .remove_lifecycle_package(&base_identity, Duration::from_secs(1))
        .await
        .unwrap();
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn verified_catalog_dependencies_must_match_the_admitted_manifest() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("knowledge");
    let fixture =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/packages/plugin-v3-okf/package");
    crate::package::copy_package(&fixture, &source)
        .await
        .unwrap();
    let manifest_path = source.join(MANIFEST_NAME);
    let manifest = fs::read_to_string(&manifest_path).await.unwrap();
    let manifest = manifest.replace(
        "  repository {",
        "  dependency \"acme/base\" {\n    version = \"^1.0.0\"\n  }\n\n  repository {",
    );
    fs::write(&manifest_path, manifest).await.unwrap();

    let (manifest, manifest_bytes) = read_manifest(&source).await.unwrap();
    let package_digest = package_sha256(&source).await.unwrap();
    let manifest_digest = sha256(&manifest_bytes);
    let mut catalog = a3s_use_core::PluginCatalogRecord::from_json(include_bytes!(
        "../../core/fixtures/plugins/catalog-record-okf-v3.json"
    ))
    .unwrap();
    catalog.target = "any".to_string();
    catalog.archive.target_name = catalog.archive.target_name.replace("linux-x86_64", "any");
    catalog.package.sha256 = Some(format!("sha256:{package_digest}"));
    catalog.package.manifest_sha256 = Some(format!("sha256:{manifest_digest}"));
    catalog.validate().unwrap();
    let verified = VerifiedPluginCatalogRecord::new(
        catalog.clone(),
        a3s_use_core::VerifiedCatalogProvenance {
            registry_name: "fixture".to_string(),
            registry_url: "https://packages.example.test/catalog/".to_string(),
            root_sha256: format!("sha256:{}", "a".repeat(64)),
            root_version: 1,
            timestamp_version: 1,
            snapshot_version: 1,
            targets_version: 1,
            catalog_record_digest: catalog.descriptor_digest().unwrap(),
        },
    )
    .unwrap();
    let resolved = ResolvedRemotePackage::from_verified_catalog(&verified).unwrap();

    let error = validate_catalog_binding(
        &verified,
        Some(&resolved),
        &manifest,
        &manifest_digest,
        &package_digest,
    )
    .unwrap_err();
    assert_eq!(error.code, "use.extension.catalog_package_mismatch");
    assert!(error.message.contains("dependency graph"));
}

#[tokio::test]
async fn lifecycle_generation_binding_fails_closed_and_snapshot_repairs_tampered_projection() {
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
    let identity = lifecycle_identity(&candidate, 13);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();

    let snapshot_path = registry.paths().registry_snapshot_path();
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).await.unwrap()).unwrap();
    snapshot["routes"][0]["lifecycleGeneration"] = serde_json::json!(99);
    fs::write(
        &snapshot_path,
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    let repaired = registry.snapshot().await.unwrap();
    assert_eq!(repaired.routes[0].lifecycle_generation, Some(13));

    let receipt_path = registry.paths().receipt_path("acme/cognitive");
    let mut receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(&receipt_path).await.unwrap()).unwrap();
    receipt["lifecycleGeneration"] = serde_json::json!(14);
    fs::write(&receipt_path, serde_json::to_vec_pretty(&receipt).unwrap())
        .await
        .unwrap();
    let error = registry.get("acme/cognitive").await.unwrap_err();
    assert!(matches!(
        error.code.as_str(),
        "use.extension.lifecycle_receipt_invalid" | "use.extension.ownership_invalid"
    ));
}

#[tokio::test]
async fn lifecycle_commit_repairs_crashes_after_root_or_receipt_commit() {
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
    let identity = lifecycle_identity(&candidate, 15);
    let registry = registry(temp.path());
    let target = registry.lifecycle_package_root(&identity);

    // Model a crash after the deterministic immutable root was committed but
    // before the authoritative receipt was written.
    crate::package::copy_package(&source, &target)
        .await
        .unwrap();
    let committed = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(committed.changed);
    assert_eq!(committed.extension.receipt.package_root, target);

    // Model a second crash after receipt replacement but before snapshot
    // publication. Replaying the same checkpoint repairs only the projection.
    let snapshot_path = registry.paths().registry_snapshot_path();
    let mut snapshot: serde_json::Value =
        serde_json::from_slice(&fs::read(&snapshot_path).await.unwrap()).unwrap();
    snapshot["routes"] = serde_json::json!([]);
    fs::write(
        &snapshot_path,
        serde_json::to_vec_pretty(&snapshot).unwrap(),
    )
    .await
    .unwrap();
    let replay = registry
        .commit_lifecycle_package(&identity, &candidate)
        .await
        .unwrap();
    assert!(!replay.changed);
    assert_eq!(registry.snapshot().await.unwrap().routes.len(), 1);
}

#[tokio::test]
async fn lifecycle_commit_refuses_to_replace_a_retained_generation() {
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
    let first = lifecycle_identity(&candidate, 17);
    let next = lifecycle_identity(&candidate, 18);
    let registry = registry(temp.path());
    registry
        .commit_lifecycle_package(&first, &candidate)
        .await
        .unwrap();

    let error = registry
        .commit_lifecycle_package(&next, &candidate)
        .await
        .unwrap_err();
    assert_eq!(
        error.code,
        "use.extension.lifecycle_generation_retirement_required"
    );
    assert_eq!(
        registry
            .get("acme/cognitive")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .lifecycle_generation,
        Some(17)
    );
}

#[tokio::test]
async fn public_lifecycle_candidate_accepts_the_real_v3_host_version() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/packages/plugin-v3-cognitive/package");
    let candidate = ExtensionLifecyclePackage::prepare_local("acme/cognitive", &fixture, true)
        .await
        .unwrap();
    assert_eq!(candidate.package_id(), "acme/cognitive");
}
