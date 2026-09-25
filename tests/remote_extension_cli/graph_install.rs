use super::*;

async fn capability_intent_evidence(
    registry: ExtensionRegistry,
    route: &str,
) -> Result<(Option<u64>, Option<String>, Option<u64>, Option<bool>), a3s_use_core::UseError> {
    let snapshot = CapabilityRegistry::new(registry).snapshot().await?;
    let route_enabled = snapshot
        .capabilities
        .iter()
        .find(|capability| capability.alias.as_deref() == Some(route))
        .map(|capability| capability.enabled);
    let cursor_generation = snapshot.cursor().installation_generation;
    Ok((
        snapshot.installation_generation,
        snapshot.installation_snapshot_digest,
        cursor_generation,
        route_enabled,
    ))
}

#[test]
fn capability_snapshot_binds_the_exact_installation_enablement_intent() {
    std::thread::Builder::new()
        .name("capability-installation-intent".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap()
                .block_on(capability_snapshot_installation_intent_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn capability_snapshot_installation_intent_scenario() {
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
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let extension_registry = ExtensionRegistry::new(extension_paths(&home));
    let manager = CognitivePackageManager::new(extension_registry.clone()).unwrap();

    tokio::time::timeout(
        std::time::Duration::from_secs(45),
        manager.install_remote(
            &trusted,
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        ),
    )
    .await
    .expect("Control sole-authority install timed out")
    .expect("Control sole-authority install");
    // Control is sole mutable authority: no legacy graph leaf after install.
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(manager
        .installed_package_lock("acme/root")
        .await
        .unwrap()
        .is_some());
    assert_eq!(manager.installed_package_locks().await.unwrap().len(), 1);

    let observed = manager.observe_package("acme/root").await.unwrap();
    assert_eq!(observed.desired, PluginDesiredState::Enabled);
    assert!(observed.package_generation.is_some());
    let installed_generation = observed.package_generation.unwrap();

    let mut installed_evidence = None;
    for _ in 0..16 {
        match Box::pin(capability_intent_evidence(
            extension_registry.clone(),
            "root",
        ))
        .await
        {
            Ok(evidence) => {
                installed_evidence = Some(evidence);
                break;
            }
            Err(error) if error.code == "use.capability.registry_busy" => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => panic!("Control capability projection failed: {error:?}"),
        }
    }
    let installed_evidence =
        installed_evidence.expect("Control capability projection after install");
    assert_eq!(installed_evidence.3, Some(true));
    assert!(installed_evidence.0.is_some());
    assert_eq!(installed_evidence.2, installed_evidence.0);

    let disable = CognitivePackageEnablementRequest::new(
        "enablement:disable:capability-intent",
        "acme/root",
        installed_generation,
        false,
    )
    .unwrap();
    apply_planned_enablement(&manager, &disable).await.unwrap();

    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    let disabled = manager.observe_package("acme/root").await.unwrap();
    assert_eq!(disabled.desired, PluginDesiredState::InstalledDisabled);
    assert!(disabled.package_generation.unwrap() > installed_generation);

    let mut disabled_evidence = None;
    for _ in 0..16 {
        match Box::pin(capability_intent_evidence(
            extension_registry.clone(),
            "root",
        ))
        .await
        {
            Ok(evidence) => {
                disabled_evidence = Some(evidence);
                break;
            }
            Err(error) if error.code == "use.capability.registry_busy" => {
                tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            }
            Err(error) => panic!("Control capability projection failed: {error:?}"),
        }
    }
    let disabled_evidence = disabled_evidence.expect("Control capability projection after disable");
    assert_eq!(disabled_evidence.3, Some(false));
    assert!(disabled_evidence.0.is_some());
    assert!(disabled_evidence.0 > installed_evidence.0);
    assert_eq!(disabled_evidence.2, disabled_evidence.0);
}

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
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let extension_registry = ExtensionRegistry::new(extension_paths(&home));
    let manager = CognitivePackageManager::new(extension_registry.clone()).unwrap();

    let installed_result = manager
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
    assert_eq!(
        manager.installed_package_lock("acme/root").await.unwrap(),
        Some(installed_result.package_lock.clone())
    );
    assert_eq!(
        manager.installed_package_locks().await.unwrap(),
        vec![installed_result.package_lock.clone()]
    );
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "package-enablement").exists());

    let root_digest = installed_result.package_lock.packages[0]
        .catalog
        .record
        .package
        .sha256
        .as_ref()
        .unwrap();
    let package_root = use_paths(&home)
        .artifact_store()
        .expanded_package_path(root_digest)
        .unwrap();
    assert!(package_root.is_dir());

    let observed = manager.observe_package("acme/root").await.unwrap();
    let installed_generation = observed.package_generation.unwrap();
    assert_eq!(observed.desired, PluginDesiredState::Enabled);

    // Legacy authority beside Control must fail closed (not busy-lock overfitting).
    std::fs::create_dir_all(scoped_state(&home, "extensions")).unwrap();
    assert_eq!(
        apply_planned_enablement(
            &manager,
            &CognitivePackageEnablementRequest::new(
                "enablement:disable:legacy-authority",
                "acme/root",
                installed_generation,
                false,
            )
            .unwrap(),
        )
        .await
        .unwrap_err()
        .code,
        "use.control_store.legacy_state_unsupported"
    );
    std::fs::remove_dir_all(scoped_state(&home, "extensions")).unwrap();

    let disable = CognitivePackageEnablementRequest::new(
        "enablement:disable:0001",
        "acme/root",
        installed_generation,
        false,
    )
    .unwrap();
    let restarted = CognitivePackageManager::new(extension_registry.clone()).unwrap();
    let disabled = apply_planned_enablement(&restarted, &disable)
        .await
        .unwrap();
    assert!(disabled.changed);
    assert!(!disabled.replayed);
    let disabled_generation = disabled.state.package_generation.unwrap();
    assert!(disabled_generation > installed_generation);
    assert_eq!(
        disabled.state.desired,
        PluginDesiredState::InstalledDisabled
    );
    assert_eq!(disabled.state.observed, PluginObservedState::Installed);
    assert!(package_root.is_dir());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "operations/plugins").exists());

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
        installed_generation,
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
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
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
    assert_eq!(
        manager.observe_package("acme/root").await.unwrap().desired,
        PluginDesiredState::Enabled
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
        assert!(installed["data"]["packageGraph"]["installedPackages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|value| value.as_str() == Some(package_id)));
    }
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());

    let removed = cognitive_uninstall(&home, "acme/root");
    assert!(removed.status.success(), "{removed:?}");
    let removed = json(&removed);
    assert_eq!(
        removed["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/root", "acme/base"])
    );
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
}

#[test]
fn schema_v3_offline_install_replays_the_verified_cached_dependency_graph() {
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
    let online_lock = json(&installed)["data"]["packageGraph"]["packageLock"].clone();
    assert_eq!(json(&installed)["data"]["registryAccess"], "refreshed");
    let removed = cognitive_uninstall(&home, "acme/root");
    assert!(removed.status.success(), "{removed:?}");

    server.clear_requests();
    let reinstalled =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(reinstalled.status.success(), "{reinstalled:?}");
    let reinstalled = json(&reinstalled);
    assert_eq!(reinstalled["data"]["registryAccess"], "cached");
    assert_eq!(
        reinstalled["data"]["packageGraph"]["packageLock"],
        online_lock
    );
    assert_eq!(
        reinstalled["data"]["packageGraph"]["installedPackages"],
        serde_json::json!(["acme/base", "acme/root"])
    );
    assert!(server.requests().is_empty());
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
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());

    let second_removed = cognitive_uninstall(&home, "acme/second");
    assert!(second_removed.status.success(), "{second_removed:?}");
    assert_eq!(
        json(&second_removed)["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/second", "acme/base"])
    );
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
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
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let dependency_registry = TrustedRegistry::new(
        "dependency",
        dependency_server.base_url(),
        &dependency_repository.root_sha256,
        None,
        home.join("state/remote-registries/dependency"),
        use_paths(&home).artifact_store(),
    )
    .unwrap();
    let manager =
        CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home))).unwrap();

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
fn schema_v3_cli_resolves_dependencies_from_the_persisted_source_set() {
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
    let root_repository = TestRepository::with_targets(vec![root], 41, FUTURE);
    let dependency_repository = TestRepository::with_targets(vec![base], 43, FUTURE);
    let root_server = TestServer::start(root_repository.routes.clone());
    let dependency_server = TestServer::start(dependency_repository.routes.clone());
    let home = temp.path().join("home");

    for (name, server, repository) in [
        ("root", &root_server, &root_repository),
        ("dependency", &dependency_server, &dependency_repository),
    ] {
        let configured = Command::new(binary())
            .args([
                "registry",
                "source",
                "add",
                name,
                "--url",
                server.base_url(),
                "--trust-root",
                &repository.root_sha256,
                "--json",
            ])
            .env("A3S_USE_HOME", &home)
            .output()
            .unwrap();
        assert!(configured.status.success(), "{configured:?}");
    }

    let installed = Command::new(binary())
        .args([
            "install",
            "acme/root",
            "--registry-name",
            "root",
            "--version",
            "1.0.0",
            "--json",
        ])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(installed.status.success(), "{installed:?}");
    let installed = json(&installed);
    let packages = installed["data"]["packageGraph"]["packageLock"]["packages"]
        .as_array()
        .unwrap();
    let root = packages
        .iter()
        .find(|package| package["catalog"]["record"]["packageId"] == "acme/root")
        .unwrap();
    let dependency = packages
        .iter()
        .find(|package| package["catalog"]["record"]["packageId"] == "acme/base")
        .unwrap();
    assert_eq!(root["catalog"]["provenance"]["registryName"], "root");
    assert_eq!(
        dependency["catalog"]["provenance"]["registryName"],
        "dependency"
    );
    assert_eq!(target_request_count(&root_server), 1);
    assert_eq!(target_request_count(&dependency_server), 1);
    assert_eq!(
        installed["data"]["registrySourceRevision"]
            .as_str()
            .unwrap()
            .len(),
        64
    );
}

#[test]
fn schema_v3_install_rejects_legacy_authority_reintroduced_beside_control() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let root = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![root], 23, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let completed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(completed.status.success(), "{completed:?}");
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "extensions").exists());

    // Reintroducing a frozen legacy authority leaf beside Control must fail closed.
    std::fs::create_dir_all(scoped_state(&home, "extensions")).unwrap();
    std::fs::write(scoped_state(&home, "installation-snapshot.json"), b"{}").unwrap();
    let target_requests = target_request_count(&server);
    let rejected = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(!rejected.status.success(), "{rejected:?}");
    assert_eq!(
        json(&rejected)["error"]["code"],
        "use.control_store.legacy_state_unsupported"
    );
    assert_eq!(target_request_count(&server), target_requests);
    assert!(scoped_state(&home, "control.sqlite3").is_file());
}
