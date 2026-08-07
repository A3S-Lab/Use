use super::*;

#[tokio::test]
async fn schema_v3_enablement_is_generation_checked_durable_and_non_destructive() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let root = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![root], 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let extension_registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
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
    let installed = extension_registry.get("acme/root").await.unwrap().unwrap();
    let package_root = installed.receipt.package_root.clone();
    let artifact_generation = installed.receipt.lifecycle_generation.unwrap();
    let graph_path = home.join("state/package-graphs/acme/root.json");
    let graph_before = std::fs::read(&graph_path).unwrap();
    let observed = manager.observe_package("acme/root").await.unwrap();
    assert_eq!(observed.package_generation, Some(artifact_generation));
    assert_eq!(observed.desired, PluginDesiredState::Enabled);

    let disable = CognitivePackageEnablementRequest::new(
        "enablement:disable:0001",
        "acme/root",
        artifact_generation,
        false,
    )
    .unwrap();
    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    assert_eq!(
        apply_planned_enablement(&manager, &disable)
            .await
            .unwrap_err()
            .code,
        "use.extension.busy"
    );
    assert!(
        extension_registry
            .get("acme/root")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);

    let restarted = CognitivePackageManager::new(extension_registry.clone()).unwrap();
    let disabled = apply_planned_enablement(&restarted, &disable)
        .await
        .unwrap();
    assert!(disabled.changed);
    assert!(!disabled.replayed);
    let disabled_generation = disabled.state.package_generation.unwrap();
    assert!(disabled_generation > artifact_generation);
    assert_eq!(
        disabled.state.desired,
        PluginDesiredState::InstalledDisabled
    );
    assert_eq!(disabled.state.observed, PluginObservedState::Installed);
    assert!(extension_registry
        .find_published_route("root")
        .await
        .unwrap()
        .is_none());
    assert!(
        !extension_registry
            .get("acme/root")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );
    assert!(package_root.is_dir());
    assert_eq!(std::fs::read(&graph_path).unwrap(), graph_before);

    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(lifecycle_journal_path(&home, "acme/root")).unwrap())
            .unwrap();
    assert_eq!(journal["intent"]["action"], "disable");
    assert_eq!(
        journal["intent"]["checkpoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|checkpoint| checkpoint["kind"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["capability-hidden", "calls-drained", "surface-stopped"]
    );
    assert_eq!(journal["receipts"].as_array().unwrap().len(), 3);

    let restarted_again = CognitivePackageManager::new(extension_registry.clone()).unwrap();
    let replayed = apply_planned_enablement(&restarted_again, &disable)
        .await
        .unwrap();
    assert!(replayed.replayed);
    let mut expected_replay = disabled.clone();
    expected_replay.replayed = true;
    assert_eq!(replayed, expected_replay);

    let changed_reuse = CognitivePackageEnablementRequest::new(
        "enablement:disable:0001",
        "acme/root",
        disabled_generation,
        true,
    )
    .unwrap();
    assert_eq!(
        apply_planned_enablement(&restarted_again, &changed_reuse)
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_enablement_operation_conflict"
    );

    let stale = CognitivePackageEnablementRequest::new(
        "enablement:enable:stale",
        "acme/root",
        artifact_generation,
        true,
    )
    .unwrap();
    assert_eq!(
        apply_planned_enablement(&restarted_again, &stale)
            .await
            .unwrap_err()
            .code,
        "use.plugin.package_generation_changed"
    );
    assert!(extension_registry
        .find_published_route("root")
        .await
        .unwrap()
        .is_none());

    let enable = CognitivePackageEnablementRequest::new(
        "enablement:enable:0002",
        "acme/root",
        disabled_generation,
        true,
    )
    .unwrap();
    let enabled = apply_planned_enablement(&restarted_again, &enable)
        .await
        .unwrap();
    assert!(enabled.changed);
    assert!(enabled.state.package_generation.unwrap() > disabled_generation);
    assert_eq!(enabled.state.desired, PluginDesiredState::Enabled);
    assert_eq!(enabled.state.observed, PluginObservedState::Ready);
    assert!(extension_registry
        .find_published_route("root")
        .await
        .unwrap()
        .is_some());
    let enabled_generation = enabled.state.package_generation.unwrap();
    let no_change = CognitivePackageEnablementRequest::new(
        "enablement:enable:noop:0003",
        "acme/root",
        enabled_generation,
        true,
    )
    .unwrap();
    let no_change = restarted_again.plan_enablement(&no_change).await.unwrap();
    assert_eq!(
        no_change.status,
        CognitivePackageEnablementPlanStatus::NoChange
    );
    assert!(no_change.plan.is_none());
    assert_eq!(no_change.state.package_generation, Some(enabled_generation));
    assert!(package_root.is_dir());
    assert_eq!(std::fs::read(graph_path).unwrap(), graph_before);

    let state_generation_before_reinstall = no_change.state.package_generation.unwrap();
    restarted_again.uninstall("acme/root").await.unwrap();
    let absent = restarted_again.observe_package("acme/root").await.unwrap();
    assert_eq!(absent.desired, PluginDesiredState::Absent);
    assert!(absent.package_generation.is_none());
    restarted_again
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
    let reinstalled = restarted_again.observe_package("acme/root").await.unwrap();
    assert!(reinstalled.package_generation.unwrap() > state_generation_before_reinstall);
}

#[tokio::test]
async fn enablement_planning_distinguishes_planned_no_change_and_completed_outcomes() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let root = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![root], 7, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let extension_registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
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

    let observed = manager.observe_package("acme/root").await.unwrap();
    let disable = CognitivePackageEnablementRequest::new(
        "enablement:plan:disable:0001",
        "acme/root",
        observed.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let planned = manager.plan_enablement(&disable).await.unwrap();
    assert_eq!(
        planned.status,
        CognitivePackageEnablementPlanStatus::Planned
    );
    assert_eq!(planned.state, observed);
    let envelope = planned.plan.as_ref().unwrap();
    assert_eq!(envelope.plan.action, PluginOperationAction::Disable);
    assert_eq!(
        envelope.plan.schema,
        a3s_use_core::PLUGIN_OPERATION_PLAN_SCHEMA_V4
    );
    assert_eq!(envelope.plan.operation_id, disable.operation_id);
    assert!(planned.result.is_none());
    let canonical = planned.canonical_bytes().unwrap();
    assert_eq!(
        CognitivePackageEnablementPlanResult::from_json(&canonical).unwrap(),
        planned
    );
    assert!(
        extension_registry
            .get("acme/root")
            .await
            .unwrap()
            .unwrap()
            .receipt
            .enabled
    );

    let disabled = manager
        .apply_enablement(&disable, envelope.clone(), None)
        .await
        .unwrap();
    let completed = manager.plan_enablement(&disable).await.unwrap();
    assert_eq!(
        completed.status,
        CognitivePackageEnablementPlanStatus::Completed
    );
    assert!(completed.plan.is_some());
    assert_eq!(
        completed.result.as_ref(),
        Some(&{
            let mut replayed = disabled.clone();
            replayed.replayed = true;
            replayed
        })
    );
    assert_eq!(completed.state, disabled.state);

    let no_change = CognitivePackageEnablementRequest::new(
        "enablement:plan:disable:noop:0002",
        "acme/root",
        disabled.state.package_generation.unwrap(),
        false,
    )
    .unwrap();
    let no_change = manager.plan_enablement(&no_change).await.unwrap();
    assert_eq!(
        no_change.status,
        CognitivePackageEnablementPlanStatus::NoChange
    );
    assert!(no_change.plan.is_none());
    assert!(no_change.result.is_none());
    assert_eq!(no_change.state, disabled.state);
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
fn schema_v3_install_rejects_state_reintroduced_after_cutover_evidence_was_retired() {
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
    let rejected = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(!rejected.status.success(), "{rejected:?}");
    assert_eq!(
        json(&rejected)["error"]["code"],
        "use.extension.registry_cutover_conflict"
    );
    assert!(!graph_path.exists());
    assert!(pending_path.exists());
    assert_eq!(target_request_count(&server), target_requests);
    let journal: serde_json::Value =
        serde_json::from_slice(&std::fs::read(journal_path).unwrap()).unwrap();
    assert_eq!(journal["status"], "applying");
    assert_eq!(journal["receipts"].as_array().unwrap().len(), 2);
}
