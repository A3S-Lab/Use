#![cfg(feature = "extensions")]

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use a3s_use::cognitive_package::CognitivePackageManager;
use a3s_use_core::{
    CatalogAvailability, CatalogSurface, PluginCatalogRecord, PluginPackageDependency,
    PluginReleaseChannel, PluginSurfaceKind, PLUGIN_CATALOG_SCHEMA_V3,
};
use a3s_use_extension::{
    prepare_remote_package, ExtensionPaths, ExtensionRegistry, ResolvedRemotePackage,
    TrustedRegistry,
};
use fs2::FileExt;
use sha2::{Digest, Sha256};

#[path = "../crates/extension/src/tuf_test_support.rs"]
mod tuf_test_support;

use tuf_test_support::{
    extension_archive, package_directory_archive, TestRepository, TestServer, TestTarget, FUTURE,
    PACKAGE_VERSION,
};

const OKF_CATALOG_V3: &[u8] =
    include_bytes!("../crates/core/fixtures/plugins/catalog-record-okf-v3.json");

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
}

#[tokio::test]
async fn signed_registry_install_uses_reviewed_target_and_reports_tuf_provenance() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        temp.path().join("review-state"),
    )
    .unwrap();
    let reviewed = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let plan_digest = reviewed.resolved().plan_digest().unwrap();
    drop(reviewed);
    assert_no_target_request(&server);

    let home = temp.path().join("home");
    let installed = registry_install(&server, &repository, &home, Some(&plan_digest), &[]);
    assert!(installed.status.success(), "{installed:?}");
    let installed_json = json(&installed);
    assert_eq!(installed_json["data"]["changed"], true);
    assert_eq!(installed_json["data"]["component"]["trust"], "registry-tuf");
    assert_eq!(
        installed_json["data"]["component"]["registry"]["registryName"],
        "fixture"
    );
    assert_eq!(
        installed_json["data"]["component"]["registry"]["sha256"],
        repository.target_sha256
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("/targets/"))
            .count(),
        1
    );

    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join("state/extensions/a3s/science.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["trust"], "registry-tuf");
    let provenance: ResolvedRemotePackage =
        serde_json::from_value(receipt["registry"].clone()).unwrap();
    assert_eq!(provenance.plan_digest().unwrap(), plan_digest);

    let inspected = Command::new(binary())
        .args(["extension", "inspect", "a3s/science", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(inspected.status.success(), "{inspected:?}");
    let inspected = json(&inspected);
    assert_eq!(inspected["data"]["extension"]["trust"], "registry-tuf");
    assert_eq!(
        inspected["data"]["extension"]["registry"]["targetName"],
        repository.target_name
    );

    let second = registry_install(&server, &repository, &home, Some(&plan_digest), &[]);
    assert!(second.status.success(), "{second:?}");
    assert_eq!(json(&second)["data"]["changed"], false);
}

#[test]
fn registry_plan_mismatch_fails_before_target_download() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let output = registry_install(
        &server,
        &repository,
        &temp.path().join("home"),
        Some(&"0".repeat(64)),
        &[],
    );

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.extension.registry_plan_mismatch"
    );
    assert_no_target_request(&server);
}

#[test]
fn registry_install_rejects_unsigned_and_local_source_combinations() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");

    let unsigned = registry_install(&server, &repository, &home, None, &["--allow-unsigned"]);
    assert!(!unsigned.status.success(), "{unsigned:?}");
    assert_eq!(json(&unsigned)["error"]["code"], "use.cli.invalid_usage");

    let local = Command::new(binary())
        .args([
            "component",
            "install",
            "a3s/science",
            "--from",
            temp.path().to_str().unwrap(),
            "--allow-unsigned",
            "--registry-name",
            "fixture",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!local.status.success(), "{local:?}");
    assert_eq!(json(&local)["error"]["code"], "use.cli.invalid_usage");
    assert!(server.requests().is_empty());
}

#[test]
fn schema_v3_install_resolves_and_activates_the_complete_dependency_graph() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let base = cognitive_skill_target(temp.path(), "acme/base", "base", Vec::new(), &target);
    let root = cognitive_skill_target(
        temp.path(),
        "acme/root",
        "root",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![root, base], 11, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");
    let installed = json(&installed);
    assert_eq!(installed["data"]["changed"], true);
    assert_eq!(
        installed["data"]["packageGraph"]["packageLock"]["rootPackageId"],
        "acme/root"
    );
    assert_eq!(
        installed["data"]["packageGraph"]["installedPackages"],
        serde_json::json!(["acme/base", "acme/root"])
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("/targets/"))
            .count(),
        2
    );

    for package_id in ["acme/base", "acme/root"] {
        let receipt_path = home
            .join("state/extensions")
            .join(format!("{package_id}.json"));
        let receipt: serde_json::Value =
            serde_json::from_slice(&std::fs::read(receipt_path).unwrap()).unwrap();
        assert_eq!(receipt["schemaVersion"], 3);
        assert_eq!(receipt["enabled"], true);
        assert!(receipt["lifecycleGeneration"].as_u64().unwrap() > 0);
    }

    let removed = cognitive_uninstall(&home, "acme/root");
    assert!(removed.status.success(), "{removed:?}");
    let removed = json(&removed);
    assert_eq!(
        removed["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/root", "acme/base"])
    );
    for package_id in ["acme/base", "acme/root"] {
        assert!(!home
            .join("state/extensions")
            .join(format!("{package_id}.json"))
            .exists());
    }
}

#[test]
fn schema_v3_cli_upgrade_publishes_the_candidate_graph_and_reports_exact_transitions() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/root",
        "root",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let next = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/root",
        "root",
        "1.1.0",
        vec![PluginPackageDependency::new("acme/added", "^1.0.0").unwrap()],
        &target,
    );
    let added = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/added",
        "added",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let first_repository = TestRepository::with_targets(vec![first], 47, FUTURE);
    let next_repository = TestRepository::with_targets(vec![next, added], 53, FUTURE);
    let first_server = TestServer::start(first_repository.routes.clone());
    let next_server = TestServer::start(next_repository.routes.clone());
    let home = temp.path().join("home");

    let installed =
        cognitive_registry_install(&first_server, &first_repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");
    let upgraded = cognitive_registry_upgrade(
        &next_server,
        &next_repository,
        &home,
        "acme/root",
        "1.1.0",
        &[],
    );
    assert!(upgraded.status.success(), "{upgraded:?}");
    let upgraded = json(&upgraded);
    assert_eq!(upgraded["data"]["changed"], true);
    assert_eq!(upgraded["data"]["component"]["version"], "1.1.0");
    assert_eq!(
        upgraded["data"]["packageGraph"]["replacedPackages"],
        serde_json::json!(["acme/root"])
    );
    assert_eq!(
        upgraded["data"]["packageGraph"]["addedPackages"],
        serde_json::json!(["acme/added"])
    );
    assert_eq!(
        upgraded["data"]["packageGraph"]["plan"]["plan"]["action"],
        "upgrade"
    );

    let replay = cognitive_registry_upgrade(
        &next_server,
        &next_repository,
        &home,
        "acme/root",
        "1.1.0",
        &[],
    );
    assert!(replay.status.success(), "{replay:?}");
    assert_eq!(json(&replay)["data"]["changed"], false);
}

#[test]
fn schema_v3_uninstall_retains_a_dependency_owned_by_another_root() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let base = cognitive_skill_target(temp.path(), "acme/base", "base", Vec::new(), &target);
    let first = cognitive_skill_target(
        temp.path(),
        "acme/first",
        "first",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let second = cognitive_skill_target(
        temp.path(),
        "acme/second",
        "second",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, second, base], 13, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let first = cognitive_registry_install(&server, &repository, &home, "acme/first", &[]);
    assert!(first.status.success(), "{first:?}");
    let second = cognitive_registry_install(&server, &repository, &home, "acme/second", &[]);
    assert!(second.status.success(), "{second:?}");
    assert_eq!(
        json(&second)["data"]["packageGraph"]["retainedPackages"],
        serde_json::json!(["acme/base"])
    );

    let first_removed = cognitive_uninstall(&home, "acme/first");
    assert!(first_removed.status.success(), "{first_removed:?}");
    let first_removed = json(&first_removed);
    assert_eq!(
        first_removed["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/first"])
    );
    assert_eq!(
        first_removed["data"]["packageGraph"]["retainedPackages"],
        serde_json::json!(["acme/base"])
    );
    assert!(home.join("state/extensions/acme/base.json").exists());
    assert!(home.join("state/extensions/acme/second.json").exists());

    let second_removed = cognitive_uninstall(&home, "acme/second");
    assert!(second_removed.status.success(), "{second_removed:?}");
    assert_eq!(
        json(&second_removed)["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/second", "acme/base"])
    );
    assert!(!home.join("state/extensions/acme/base.json").exists());
}

#[tokio::test]
async fn schema_v3_manager_resolves_dependencies_from_host_injected_registries() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let base = cognitive_skill_target(temp.path(), "acme/base", "base", Vec::new(), &target);
    let root = cognitive_skill_target(
        temp.path(),
        "acme/root",
        "root",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let root_repository = TestRepository::with_targets(vec![root], 31, FUTURE);
    let dependency_repository = TestRepository::with_targets(vec![base], 37, FUTURE);
    let root_server = TestServer::start(root_repository.routes.clone());
    let dependency_server = TestServer::start(dependency_repository.routes.clone());
    let home = temp.path().join("home");
    let root_registry = TrustedRegistry::new(
        "root",
        root_server.base_url(),
        &root_repository.root_sha256,
        None,
        home.join("state/remote-registries/root"),
    )
    .unwrap();
    let dependency_registry = TrustedRegistry::new(
        "dependency",
        dependency_server.base_url(),
        &dependency_repository.root_sha256,
        None,
        home.join("state/remote-registries/dependency"),
    )
    .unwrap();
    let manager = CognitivePackageManager::new(ExtensionRegistry::new(ExtensionPaths::new(
        home.join("data"),
        home.join("state"),
    )))
    .unwrap();

    let installed = manager
        .install_remote(
            &root_registry,
            &[dependency_registry],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert_eq!(installed.installed_packages, ["acme/base", "acme/root"]);
    assert_eq!(
        installed
            .package_lock
            .package("acme/root")
            .unwrap()
            .catalog
            .provenance
            .registry_name,
        "root"
    );
    assert_eq!(
        installed
            .package_lock
            .package("acme/base")
            .unwrap()
            .catalog
            .provenance
            .registry_name,
        "dependency"
    );
    assert_eq!(target_request_count(&root_server), 1);
    assert_eq!(target_request_count(&dependency_server), 1);
}

#[tokio::test]
async fn schema_v3_manager_upgrades_one_exact_graph_and_retires_the_prior_generation() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first_target = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/root",
        "root",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let base_target = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/base",
        "base",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let next_target = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/root",
        "root",
        "1.1.0",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let third_target = cognitive_skill_target_version(
        &temp.path().join("third"),
        "acme/root",
        "root",
        "1.2.0",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let first_repository =
        TestRepository::with_targets(vec![first_target, base_target], 41, FUTURE);
    let next_repository = TestRepository::with_targets(vec![next_target], 43, FUTURE);
    let third_repository = TestRepository::with_targets(vec![third_target], 45, FUTURE);
    let first_server = TestServer::start(first_repository.routes.clone());
    let next_server = TestServer::start(next_repository.routes.clone());
    let third_server = TestServer::start(third_repository.routes.clone());
    let home = temp.path().join("home");
    let first_registry = TrustedRegistry::new(
        "first",
        first_server.base_url(),
        &first_repository.root_sha256,
        None,
        home.join("state/remote-registries/first"),
    )
    .unwrap();
    let next_registry = TrustedRegistry::new(
        "next",
        next_server.base_url(),
        &next_repository.root_sha256,
        None,
        home.join("state/remote-registries/next"),
    )
    .unwrap();
    let third_registry = TrustedRegistry::new(
        "third",
        third_server.base_url(),
        &third_repository.root_sha256,
        None,
        home.join("state/remote-registries/third"),
    )
    .unwrap();
    let extension_registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
    let manager = CognitivePackageManager::new(extension_registry.clone()).unwrap();
    let installed = manager
        .install_remote(
            &first_registry,
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    let prior_generation = installed.root.receipt.lifecycle_generation.unwrap();

    let upgraded = manager
        .upgrade_remote(
            &next_registry,
            std::slice::from_ref(&first_registry),
            "acme/root",
            Some("1.1.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(upgraded.changed);
    assert_eq!(upgraded.root.manifest.version, "1.1.0");
    assert_eq!(upgraded.replaced_packages, ["acme/root"]);
    assert!(upgraded.added_packages.is_empty());
    assert_eq!(upgraded.retained_packages, ["acme/base"]);
    assert_eq!(
        upgraded.plan.as_ref().unwrap().plan.action,
        a3s_use_core::PluginOperationAction::Upgrade
    );
    assert!(
        upgraded.root.receipt.lifecycle_generation.unwrap() > prior_generation,
        "the replacement must advance the exact lifecycle generation"
    );
    let prior_state = upgraded
        .prior_package_lock
        .package("acme/root")
        .unwrap()
        .catalog
        .selected_state(&[])
        .unwrap();
    let prior_identity = a3s_use_extension::ExtensionLifecycleIdentity::new(
        "acme/root",
        prior_state.release.package_sha256,
        prior_state.release.manifest_sha256,
        prior_generation,
    )
    .unwrap();
    assert!(extension_registry
        .get_lifecycle_generation(&prior_identity)
        .await
        .unwrap()
        .is_none());
    assert!(!home
        .join("state/operations/package-graphs/upgrade/acme/root.json")
        .exists());
    let graph: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join("state/package-graphs/acme/root.json")).unwrap(),
    )
    .unwrap();
    let root_graph = graph["packageLock"]["packages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|package| package["catalog"]["record"]["packageId"] == "acme/root")
        .unwrap();
    assert_eq!(root_graph["catalog"]["record"]["version"], "1.1.0");

    let replay = manager
        .upgrade_remote(
            &next_registry,
            std::slice::from_ref(&first_registry),
            "acme/root",
            Some("1.1.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(!replay.changed);
    assert!(replay.plan.is_none());

    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    let interrupted = manager
        .upgrade_remote(
            &third_registry,
            std::slice::from_ref(&first_registry),
            "acme/root",
            Some("1.2.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(interrupted.code, "use.extension.busy");
    assert_eq!(interrupted.details["rollbackCode"], "use.extension.busy");
    assert!(home
        .join("state/operations/package-graphs/upgrade/acme/root.json")
        .exists());
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let recovered = manager
        .upgrade_remote(
            &third_registry,
            std::slice::from_ref(&first_registry),
            "acme/root",
            Some("1.2.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap_err();
    assert_eq!(
        recovered.code,
        "use.plugin.package_graph_upgrade_rolled_back"
    );
    assert!(!home
        .join("state/operations/package-graphs/upgrade/acme/root.json")
        .exists());
    assert_eq!(
        extension_registry
            .get("acme/root")
            .await
            .unwrap()
            .unwrap()
            .manifest
            .version,
        "1.1.0"
    );

    let third = manager
        .upgrade_remote(
            &third_registry,
            std::slice::from_ref(&first_registry),
            "acme/root",
            Some("1.2.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    assert!(third.changed);
    assert_eq!(third.root.manifest.version, "1.2.0");
}

#[test]
fn schema_v3_okf_install_fails_before_any_package_becomes_visible_without_knowledge() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let archive = package_directory_archive(&package_root);
    let mut catalog: serde_json::Value = serde_json::from_slice(OKF_CATALOG_V3).unwrap();
    catalog["target"] = serde_json::json!(&target);
    catalog["archive"]["targetName"] = serde_json::json!(format!(
        "extensions/acme/knowledge/1.0.0/stable/{target}/acme-knowledge-1.0.0-{target}.tar.gz"
    ));
    let target_name = catalog["archive"]["targetName"]
        .as_str()
        .unwrap()
        .to_string();
    let repository =
        TestRepository::with_target_metadata(archive, target_name, catalog, 17, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let output = cognitive_registry_install(&server, &repository, &home, "acme/knowledge", &[]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.plugin.okf_provider_required"
    );
    assert!(!home.join("state/extensions/acme/knowledge.json").exists());
    let snapshot = home.join("state/registry.json");
    if snapshot.exists() {
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(snapshot).unwrap()).unwrap();
        assert_eq!(snapshot["routes"], serde_json::json!([]));
    }
}

#[test]
fn schema_v3_lock_mismatch_fails_before_any_archive_download() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let root = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![root], 19, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let output = cognitive_registry_install(
        &server,
        &repository,
        &home,
        "acme/root",
        &["--package-lock-digest", &"0".repeat(64)],
    );
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.plugin.package_lock_mismatch"
    );
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}

#[test]
fn schema_v3_install_adopts_a_published_graph_and_clears_stale_pending_evidence() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let root = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![root], 23, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let pending_path = home.join("state/operations/package-graphs/install/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");

    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    let interrupted = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(!interrupted.status.success(), "{interrupted:?}");
    assert_eq!(json(&interrupted)["error"]["code"], "use.extension.busy");
    let pending = std::fs::read(&pending_path).unwrap();
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let completed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(completed.status.success(), "{completed:?}");
    assert!(graph_path.exists());
    assert!(!pending_path.exists());

    std::fs::remove_file(&graph_path).unwrap();
    std::fs::write(&pending_path, pending).unwrap();
    let journal_path = lifecycle_journal_path(&home, "acme/root");
    let mut journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&journal_path).unwrap()).unwrap();
    assert_eq!(journal["status"], "completed");
    assert_eq!(
        journal["receipts"].as_array_mut().unwrap().pop().unwrap()["sequence"],
        3
    );
    journal["status"] = serde_json::json!("applying");
    journal.as_object_mut().unwrap().remove("completedAtMs");
    std::fs::write(&journal_path, serde_json::to_vec_pretty(&journal).unwrap()).unwrap();
    let target_requests = target_request_count(&server);
    let recovered = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert_eq!(json(&recovered)["data"]["changed"], false);
    assert!(graph_path.exists());
    assert!(!pending_path.exists());
    assert_eq!(target_request_count(&server), target_requests);
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path).unwrap()).unwrap();
    assert_eq!(journal["status"], "completed");
    assert_eq!(journal["receipts"].as_array().unwrap().len(), 3);
}

#[cfg(unix)]
#[test]
fn schema_v3_uninstall_replays_after_the_root_and_graph_record_are_removed() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let base = cognitive_skill_target(temp.path(), "acme/base", "base", Vec::new(), &target);
    let root = cognitive_skill_target(
        temp.path(),
        "acme/root",
        "root",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![root, base], 29, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let root_receipt = home.join("state/extensions/acme/root.json");
    let base_receipt = home.join("state/extensions/acme/base.json");
    let pending_path = home.join("state/operations/package-graphs/uninstall/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");
    let base_generation =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&base_receipt).unwrap())
            .unwrap()["lifecycleGeneration"]
            .as_u64()
            .unwrap();
    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks/acme/base")
            .join(format!("{base_generation:020}.lock")),
    );
    let mut interrupted = Command::new(binary())
        .args(["uninstall", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_dependency_drain = wait_until(Duration::from_secs(10), || {
        !root_receipt.exists() && base_receipt.exists() && pending_path.exists()
    });
    if !reached_dependency_drain {
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        FileExt::unlock(&route_lock).unwrap();
        panic!("uninstall did not reach the dependency drain checkpoint");
    }
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);

    assert!(!root_receipt.exists());
    assert!(base_receipt.exists());
    assert!(pending_path.exists());
    std::fs::remove_file(&graph_path).unwrap();

    let recovered = cognitive_uninstall(&home, "acme/root");
    assert!(recovered.status.success(), "{recovered:?}");
    assert_eq!(
        json(&recovered)["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/root", "acme/base"])
    );
    assert!(!root_receipt.exists());
    assert!(!base_receipt.exists());
    assert!(!graph_path.exists());
    assert!(!pending_path.exists());
}

fn registry_install(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    plan_digest: Option<&str>,
    extra: &[&str],
) -> Output {
    let mut command = Command::new(binary());
    command.args([
        "component",
        "install",
        "a3s/science",
        "--registry-name",
        "fixture",
        "--registry-url",
        server.base_url(),
        "--trust-root",
        &repository.root_sha256,
    ]);
    if let Some(plan_digest) = plan_digest {
        command.args(["--registry-plan-digest", plan_digest]);
    }
    command
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn cognitive_registry_install(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    package_id: &str,
    extra: &[&str],
) -> Output {
    Command::new(binary())
        .args([
            "install",
            package_id,
            "--registry-name",
            "fixture",
            "--registry-url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
            "--version",
            "1.0.0",
        ])
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn cognitive_uninstall(home: &std::path::Path, package_id: &str) -> Output {
    Command::new(binary())
        .args(["uninstall", package_id, "--json"])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn cognitive_registry_upgrade(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    package_id: &str,
    version: &str,
    extra: &[&str],
) -> Output {
    Command::new(binary())
        .args([
            "upgrade",
            package_id,
            "--registry-name",
            "fixture",
            "--registry-url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
            "--version",
            version,
        ])
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn exclusive_lock(path: &std::path::Path) -> File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    FileExt::lock_exclusive(&file).unwrap();
    file
}

fn wait_until(timeout: Duration, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

fn target_request_count(server: &TestServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|request| request.starts_with("/targets/"))
        .count()
}

fn lifecycle_journal_path(home: &std::path::Path, package_id: &str) -> std::path::PathBuf {
    let scope = format!("{:x}", Sha256::digest(b"user/current"));
    home.join("state/operations/plugins")
        .join(scope)
        .join(package_id)
        .join("active.json")
}

fn cognitive_skill_target(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    dependencies: Vec<PluginPackageDependency>,
    target: &str,
) -> TestTarget {
    cognitive_skill_target_version(
        fixture_root,
        package_id,
        route,
        "1.0.0",
        dependencies,
        target,
    )
}

fn cognitive_skill_target_version(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    dependencies: Vec<PluginPackageDependency>,
    target: &str,
) -> TestTarget {
    let package_root = fixture_root.join("packages").join(route);
    std::fs::create_dir_all(package_root.join("skills/main")).unwrap();
    let dependency_blocks = dependencies
        .iter()
        .map(|dependency| {
            format!(
                "\n  dependency \"{}\" {{\n    version = \"{}\"\n  }}\n",
                dependency.package_id, dependency.version_requirement
            )
        })
        .collect::<String>();
    let manifest = format!(
        "extension \"{package_id}\" {{\n  schema_version = 3\n  version = \"{version}\"\n  route = \"{route}\"\n  requires_use = \">=0.3.0, <0.4.0\"\n  actions = [\"read\"]\n{dependency_blocks}\n  repository {{\n    url = \"https://github.com/acme/{route}\"\n    revision = \"0123456789abcdef0123456789abcdef01234567\"\n  }}\n\n  skill \"main\" {{\n    path = \"skills/main/SKILL.md\"\n    requires_tool = []\n    requires_mcp = []\n    requires_okf = []\n    optional = false\n  }}\n}}\n"
    );
    std::fs::write(package_root.join("a3s-use-extension.acl"), &manifest).unwrap();
    std::fs::write(
        package_root.join("README.md"),
        format!("# {package_id}\n\nCognitive package integration fixture.\n"),
    )
    .unwrap();
    std::fs::write(
        package_root.join("skills/main/SKILL.md"),
        format!("---\nname: {route}\ndescription: Cognitive package fixture\n---\n# {route}\n"),
    )
    .unwrap();

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let manifest_sha256 = format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()));
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    catalog.package_id = package_id.to_string();
    catalog.display_name = format!("{route} fixture");
    catalog.description = format!("Cognitive package fixture for {package_id}.");
    catalog.publisher = "acme".to_string();
    catalog.keywords = vec!["fixture".to_string()];
    catalog.categories = vec!["test".to_string()];
    catalog.version = version.to_string();
    catalog.channel = PluginReleaseChannel::Stable;
    catalog.requires_use = ">=0.3.0, <0.4.0".to_string();
    catalog.dependencies = dependencies;
    catalog.target = target.to_string();
    catalog.surfaces = vec![CatalogSurface {
        kind: PluginSurfaceKind::Skill,
        id: "main".to_string(),
        optional: false,
        workload: None,
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    }];
    catalog.permission_ceiling.surfaces.clear();
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    catalog.planning = None;
    catalog.archive.target_name = format!(
        "extensions/{package_id}/{version}/stable/{target}/{route}-{version}-{target}.tar.gz"
    );
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.package.manifest_sha256 = Some(manifest_sha256);
    catalog.license = "MIT".to_string();
    catalog.repository = format!("https://github.com/acme/{route}");
    catalog.availability = CatalogAvailability::Available;
    catalog.validate().unwrap();

    TestTarget {
        target_name: catalog.archive.target_name.clone(),
        custom: Some(serde_json::to_value(catalog).unwrap()),
        archive,
    }
}

fn package_fingerprint(root: &std::path::Path) -> (String, u64, u64) {
    fn collect(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut Vec<(String, std::path::PathBuf)>,
    ) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    path,
                ));
            }
        }
    }

    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update(b"a3s-use-expanded-package-v1\0");
    let mut expanded_bytes = 0_u64;
    for (relative, path) in &files {
        let size = std::fs::metadata(path).unwrap().len();
        expanded_bytes += size;
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update(size.to_be_bytes());
        let mut input = std::fs::File::open(path).unwrap();
        let mut buffer = Vec::new();
        input.read_to_end(&mut buffer).unwrap();
        digest.update(buffer);
    }
    (
        format!("{:x}", digest.finalize()),
        files.len() as u64,
        expanded_bytes,
    )
}

fn host_target() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        (os, arch) => panic!("unsupported test target {os}-{arch}"),
    }
    .to_string()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON output ({error}): stdout={:?}, stderr={:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_no_target_request(server: &TestServer) {
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}
