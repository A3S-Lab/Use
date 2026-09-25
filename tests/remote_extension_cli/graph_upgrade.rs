use super::*;

#[tokio::test]
async fn schema_v3_upgrade_advances_enablement_state_without_reusing_artifact_generation() {
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
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, next], 43, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let extension_registry = ExtensionRegistry::new(extension_paths(&home));
    let manager = CognitivePackageManager::new(extension_registry.clone()).unwrap();
    manager
        .install_remote(
            &trusted,
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();

    let installed = manager.observe_package("acme/root").await.unwrap();
    let disable = CognitivePackageEnablementRequest::new(
        "enablement:upgrade:disable:0001",
        "acme/root",
        installed.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let disabled = apply_planned_enablement(&manager, &disable).await.unwrap();
    let enable = CognitivePackageEnablementRequest::new(
        "enablement:upgrade:enable:0002",
        "acme/root",
        disabled.state.package_generation.unwrap(),
        true,
    )
    .unwrap();
    let before_upgrade = apply_planned_enablement(&manager, &enable).await.unwrap();
    let state_generation_before = before_upgrade.state.package_generation.unwrap();

    manager
        .upgrade_remote(
            &trusted,
            &[],
            "acme/root",
            Some("1.1.0"),
            PluginReleaseChannel::Stable,
            None,
        )
        .await
        .unwrap();
    let upgraded_extension = manager
        .installed_extension("acme/root")
        .await
        .unwrap()
        .unwrap();
    assert_eq!(upgraded_extension.receipt.version, "1.1.0");
    let upgraded = manager.observe_package("acme/root").await.unwrap();
    assert!(upgraded.package_generation.unwrap() > state_generation_before);
    assert_eq!(upgraded.version.as_deref(), Some("1.1.0"));
    assert_eq!(upgraded.desired, PluginDesiredState::Enabled);
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
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
    let replaced = replace_registry(&next_server, &next_repository, &home);
    assert!(replaced.status.success(), "{replaced:?}");
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
    assert!(!scoped_state(&home, "operations/package-downloads/upgrade/acme/root.json").exists());

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

#[tokio::test(flavor = "multi_thread")]
async fn schema_v3_cli_upgrade_uses_only_verified_cached_targets_when_offline() {
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
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, next], 59, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");
    let configured = a3s_use_extension::RegistrySourceStore::new(use_paths(&home))
        .resolve(Some("fixture"))
        .await
        .unwrap();
    let trusted = configured.root().clone();
    prepare_remote_package(&trusted, "acme/root", Some("1.1.0"), "stable", None)
        .await
        .unwrap()
        .download()
        .await
        .unwrap();

    server.clear_requests();
    let upgraded = cognitive_registry_upgrade(
        &server,
        &repository,
        &home,
        "acme/root",
        "1.1.0",
        &["--offline"],
    );
    assert!(upgraded.status.success(), "{upgraded:?}");
    let upgraded = json(&upgraded);
    assert_eq!(upgraded["data"]["registryAccess"], "cached");
    assert_eq!(upgraded["data"]["component"]["version"], "1.1.0");
    assert_eq!(
        upgraded["data"]["packageGraph"]["replacedPackages"],
        serde_json::json!(["acme/root"])
    );
    assert!(server.requests().is_empty());
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
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
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
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
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
    let installed_packages = json(&installed)["data"]["packageGraph"]["installedPackages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(installed_packages
        .iter()
        .any(|package| package.as_str() == Some("acme/obsolete")));
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "registry.json").exists());

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
        "a3s.use.plugin-operation-plan.v4"
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
    let lock_packages = upgraded["data"]["packageGraph"]["packageLock"]["packages"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(lock_packages
        .iter()
        .all(|package| { package["catalog"]["record"]["packageId"] != "acme/obsolete" }));
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(scoped_state(&home, "control.sqlite3").is_file());

    let replayed =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/root", "1.1.0", &[]);
    assert!(replayed.status.success(), "{replayed:?}");
    let replayed = json(&replayed);
    assert_eq!(replayed["data"]["changed"], false);
    assert_eq!(
        replayed["data"]["packageGraph"]["removedPackages"],
        serde_json::json!([])
    );
    assert_eq!(
        replayed["data"]["packageGraph"]["retainedPackages"],
        serde_json::json!(["acme/base", "acme/root"])
    );
    assert_eq!(
        replayed["data"]["packageGraph"]["plan"],
        serde_json::Value::Null
    );
    assert_eq!(replayed["data"]["pluginManager"]["plan"]["replayed"], true);
    assert_eq!(
        replayed["data"]["pluginManager"]["result"]["replayed"],
        true
    );
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
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
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
    let first_lock_before = json(&first)["data"]["packageGraph"]["packageLock"].clone();
    let second_lock_before = json(&second)["data"]["packageGraph"]["packageLock"].clone();
    assert!(scoped_state(&home, "control.sqlite3").is_file());

    let upgraded =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/first", "1.1.0", &[]);
    assert!(!upgraded.status.success(), "{upgraded:?}");
    assert_eq!(
        json(&upgraded)["error"]["code"],
        "use.plugin.package_graph_shared_upgrade_required"
    );
    // Rejected upgrade must not create legacy authority or pending upgrade leaves.
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
    assert!(!scoped_state(&home, "operations/package-graphs").exists());
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    let _ = (first_lock_before, second_lock_before);
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
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let next_registry = TrustedRegistry::new(
        "next",
        next_server.base_url(),
        &next_repository.root_sha256,
        None,
        home.join("state/remote-registries/next"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let third_registry = TrustedRegistry::new(
        "third",
        third_server.base_url(),
        &third_repository.root_sha256,
        None,
        home.join("state/remote-registries/third"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let extension_registry = ExtensionRegistry::new(extension_paths(&home));
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
    assert!(!scoped_state(&home, "operations/package-graphs").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    let upgraded_lock = manager
        .installed_package_lock("acme/root")
        .await
        .unwrap()
        .unwrap();
    let root_package = upgraded_lock
        .packages
        .iter()
        .find(|package| package.catalog.record.package_id == "acme/root")
        .unwrap();
    assert_eq!(root_package.catalog.record.version, "1.1.0");

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

    // Reintroducing legacy authority beside Control must fail closed.
    std::fs::create_dir_all(scoped_state(&home, "extensions")).unwrap();
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
    assert_eq!(
        interrupted.code,
        "use.control_store.legacy_state_unsupported"
    );
    std::fs::remove_dir_all(scoped_state(&home, "extensions")).unwrap();
    assert!(!scoped_state(&home, "operations/package-graphs").exists());
    assert_eq!(
        manager
            .installed_extension("acme/root")
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
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
}
