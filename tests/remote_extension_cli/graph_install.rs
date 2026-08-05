use super::*;

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
