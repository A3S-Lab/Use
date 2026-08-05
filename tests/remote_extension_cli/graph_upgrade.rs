use super::*;

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
fn schema_v3_cli_upgrade_reuses_an_exact_dependency_owned_by_another_root() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let owner = cognitive_skill_target_version(
        &temp.path().join("owner"),
        "acme/owner",
        "owner",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/shared", "^1.0.0").unwrap()],
        &target,
    );
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
        vec![PluginPackageDependency::new("acme/shared", "^1.0.0").unwrap()],
        &target,
    );
    let shared = cognitive_skill_target_version(
        &temp.path().join("shared"),
        "acme/shared",
        "shared",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![owner, first, next, shared], 57, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let owner = cognitive_registry_install(&server, &repository, &home, "acme/owner", &[]);
    assert!(owner.status.success(), "{owner:?}");
    let first = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(first.status.success(), "{first:?}");
    let shared_receipt_path = home.join("state/extensions/acme/shared.json");
    let shared_receipt_before = std::fs::read(&shared_receipt_path).unwrap();
    let target_requests_before = target_request_count(&server);

    let upgraded =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/root", "1.1.0", &[]);
    assert!(upgraded.status.success(), "{upgraded:?}");
    let upgraded = json(&upgraded);
    assert_eq!(
        upgraded["data"]["packageGraph"]["addedPackages"],
        serde_json::json!([])
    );
    assert_eq!(
        upgraded["data"]["packageGraph"]["replacedPackages"],
        serde_json::json!(["acme/root"])
    );
    assert_eq!(
        upgraded["data"]["packageGraph"]["retainedPackages"],
        serde_json::json!(["acme/shared"])
    );
    assert!(upgraded["data"]["packageGraph"]["plan"]["plan"]["packages"]
        .as_array()
        .is_some_and(|packages| packages.iter().any(|package| {
            package["packageId"] == "acme/shared" && package["change"] == "retain"
        })));
    assert_eq!(
        std::fs::read(&shared_receipt_path).unwrap(),
        shared_receipt_before
    );
    assert_eq!(target_request_count(&server), target_requests_before + 1);
}

#[test]
fn schema_v3_cli_upgrade_removes_an_unreferenced_dependency_node() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/root",
        "root",
        "1.0.0",
        vec![
            PluginPackageDependency::new("acme/base", "^1.0.0").unwrap(),
            PluginPackageDependency::new("acme/obsolete", "^1.0.0").unwrap(),
        ],
        &target,
    );
    let next = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/root",
        "root",
        "1.1.0",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let base = cognitive_skill_target_version(
        &temp.path().join("dependencies"),
        "acme/base",
        "base",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let obsolete = cognitive_skill_target_version(
        &temp.path().join("dependencies"),
        "acme/obsolete",
        "obsolete",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, next, base, obsolete], 59, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");
    let obsolete_receipt = home.join("state/extensions/acme/obsolete.json");
    assert!(obsolete_receipt.exists());
    let generation_before = serde_json::from_slice::<serde_json::Value>(
        &std::fs::read(home.join("state/registry.json")).unwrap(),
    )
    .unwrap()["generation"]
        .as_u64()
        .unwrap();

    let upgraded =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/root", "1.1.0", &[]);
    assert!(upgraded.status.success(), "{upgraded:?}");
    let upgraded = json(&upgraded);
    assert_eq!(
        upgraded["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/obsolete"])
    );
    assert_eq!(
        upgraded["data"]["packageGraph"]["retainedPackages"],
        serde_json::json!(["acme/base"])
    );
    assert!(upgraded["data"]["packageGraph"]["plan"]["plan"]["packages"]
        .as_array()
        .is_some_and(|packages| packages.iter().any(|package| {
            package["packageId"] == "acme/obsolete" && package["change"] == "remove"
        })));
    assert_eq!(
        upgraded["data"]["packageGraph"]["plan"]["plan"]["schema"],
        "a3s.use.plugin-operation-plan.v3"
    );
    assert!(
        upgraded["data"]["packageGraph"]["plan"]["plan"]["priorPackageLockDigest"]
            .as_str()
            .is_some()
    );
    assert!(
        upgraded["data"]["packageGraph"]["plan"]["priorPackageLock"]["packages"]
            .as_array()
            .is_some_and(|packages| packages.len() == 3)
    );
    assert!(!obsolete_receipt.exists());

    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("state/registry.json")).unwrap()).unwrap();
    assert_eq!(snapshot["generation"], generation_before + 1);
    assert!(snapshot["routes"].as_array().is_some_and(|routes| routes
        .iter()
        .all(|route| route["packageId"] != "acme/obsolete")));
    let graph: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join("state/package-graphs/acme/root.json")).unwrap(),
    )
    .unwrap();
    assert!(graph["packageLock"]["packages"]
        .as_array()
        .is_some_and(|packages| packages
            .iter()
            .all(|package| { package["catalog"]["record"]["packageId"] != "acme/obsolete" })));
}

#[test]
fn schema_v3_cli_upgrade_retains_a_removed_node_owned_by_another_root() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/first",
        "first",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/obsolete", "^1.0.0").unwrap()],
        &target,
    );
    let next = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/first",
        "first",
        "1.1.0",
        Vec::new(),
        &target,
    );
    let second = cognitive_skill_target_version(
        &temp.path().join("second"),
        "acme/second",
        "second",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/obsolete", "^1.0.0").unwrap()],
        &target,
    );
    let obsolete = cognitive_skill_target_version(
        &temp.path().join("dependency"),
        "acme/obsolete",
        "obsolete",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, next, second, obsolete], 61, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let first = cognitive_registry_install(&server, &repository, &home, "acme/first", &[]);
    assert!(first.status.success(), "{first:?}");
    let second = cognitive_registry_install(&server, &repository, &home, "acme/second", &[]);
    assert!(second.status.success(), "{second:?}");

    let upgraded =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/first", "1.1.0", &[]);
    assert!(upgraded.status.success(), "{upgraded:?}");
    let upgraded = json(&upgraded);
    assert_eq!(
        upgraded["data"]["packageGraph"]["removedPackages"],
        serde_json::json!([])
    );
    assert_eq!(
        upgraded["data"]["packageGraph"]["retainedPackages"],
        serde_json::json!(["acme/obsolete"])
    );
    assert!(upgraded["data"]["packageGraph"]["plan"]["plan"]["packages"]
        .as_array()
        .is_some_and(|packages| packages.iter().any(|package| {
            package["packageId"] == "acme/obsolete" && package["change"] == "retain"
        })));
    assert!(home.join("state/extensions/acme/obsolete.json").exists());
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(home.join("state/registry.json")).unwrap()).unwrap();
    assert!(snapshot["routes"].as_array().is_some_and(|routes| routes
        .iter()
        .any(|route| route["packageId"] == "acme/obsolete" && route["enabled"] == true)));
}

#[test]
fn schema_v3_cli_upgrade_rejects_replacing_a_dependency_locked_by_another_root() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_skill_target_version(
        &temp.path().join("first-v1"),
        "acme/first",
        "first",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let next = cognitive_skill_target_version(
        &temp.path().join("first-v2"),
        "acme/first",
        "first",
        "1.1.0",
        vec![PluginPackageDependency::new("acme/base", "^2.0.0").unwrap()],
        &target,
    );
    let second = cognitive_skill_target_version(
        &temp.path().join("second"),
        "acme/second",
        "second",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let base_v1 = cognitive_skill_target_version(
        &temp.path().join("base-v1"),
        "acme/base",
        "base",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let base_v2 = cognitive_skill_target_version(
        &temp.path().join("base-v2"),
        "acme/base",
        "base",
        "2.0.0",
        Vec::new(),
        &target,
    );
    let repository =
        TestRepository::with_targets(vec![first, next, second, base_v1, base_v2], 63, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let first = cognitive_registry_install(&server, &repository, &home, "acme/first", &[]);
    assert!(first.status.success(), "{first:?}");
    let second = cognitive_registry_install(&server, &repository, &home, "acme/second", &[]);
    assert!(second.status.success(), "{second:?}");
    let snapshot_before = std::fs::read(home.join("state/registry.json")).unwrap();
    let receipt_before = std::fs::read(home.join("state/extensions/acme/base.json")).unwrap();

    let upgraded =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/first", "1.1.0", &[]);
    assert!(!upgraded.status.success(), "{upgraded:?}");
    assert_eq!(
        json(&upgraded)["error"]["code"],
        "use.plugin.package_graph_shared_upgrade_required"
    );
    assert_eq!(
        std::fs::read(home.join("state/registry.json")).unwrap(),
        snapshot_before
    );
    assert_eq!(
        std::fs::read(home.join("state/extensions/acme/base.json")).unwrap(),
        receipt_before
    );
    assert!(!home
        .join("state/operations/package-graphs/upgrade/acme/first.json")
        .exists());
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
