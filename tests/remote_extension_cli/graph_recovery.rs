use super::*;

const GRAPH_DEPENDENCY_COUNT: usize = 8;

/// Control-native graph recovery: sole-authority install, process reopen, and
/// offline exact replay without generation inflation.
///
/// Legacy kill-mid-journal checkpoints (`operations/package-graphs/`,
/// `registry.json`, lifecycle journals) are obsolete under Control. The A2
/// commit/drain failure boundary is covered by
/// `production_effects_pending_survives_restart_and_resumes_without_generation_inflation`.
#[test]
fn control_graph_install_reopens_and_replays_exact_cutover_offline_without_generation_inflation() {
    std::thread::Builder::new()
        .name("control-graph-recovery".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap()
                .block_on(control_graph_recovery_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn control_graph_recovery_scenario() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = (0..GRAPH_DEPENDENCY_COUNT)
        .map(|index| format!("acme/leaf-{index:02}"))
        .chain(std::iter::once("acme/root".to_owned()))
        .collect::<std::collections::BTreeSet<_>>();
    let dependencies = (0..GRAPH_DEPENDENCY_COUNT)
        .map(|index| {
            PluginPackageDependency::new(format!("acme/leaf-{index:02}"), "^1.0.0").unwrap()
        })
        .collect::<Vec<_>>();
    let mut targets = dependencies
        .iter()
        .enumerate()
        .map(|(index, dependency)| {
            cognitive_skill_target(
                &temp.path().join(format!("leaf-{index:02}")),
                &dependency.package_id,
                &format!("leaf-{index:02}"),
                Vec::new(),
                &target,
            )
        })
        .collect::<Vec<_>>();
    targets.push(cognitive_skill_target(
        &temp.path().join("root"),
        "acme/root",
        "root",
        dependencies,
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 103, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(&server, &repository, &home, &[]);
    server.clear_requests();

    let extension_registry = ExtensionRegistry::new(extension_paths(&home));
    let manager = CognitivePackageManager::new(extension_registry.clone()).unwrap();
    let first = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        manager.install_remote(
            &TrustedRegistry::new(
                "fixture",
                server.base_url(),
                &repository.root_sha256,
                None,
                home.join("state/remote-registries/fixture"),
                use_paths(&home).artifact_store(),
            )
            .unwrap(),
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        ),
    )
    .await
    .expect("Control graph install timed out")
    .expect("Control graph install");
    assert!(first.changed);
    assert_eq!(
        first
            .installed_packages
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>(),
        expected_package_ids
    );

    // Control is sole mutable authority after cutover.
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "operations/package-graphs").exists());

    let locks = manager.installed_package_locks().await.unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].root_package_id, "acme/root");
    let observed = manager.observe_package("acme/root").await.unwrap();
    assert_eq!(observed.desired, PluginDesiredState::Enabled);
    let installed_generation = observed.package_generation.expect("package generation");

    // Process reopen: new manager against the same Control database.
    let reopened =
        CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home))).unwrap();
    let reopened_locks = reopened.installed_package_locks().await.unwrap();
    assert_eq!(reopened_locks.len(), 1);
    assert_eq!(reopened_locks[0].root_package_id, "acme/root");
    let reopened_observed = reopened.observe_package("acme/root").await.unwrap();
    assert_eq!(reopened_observed.desired, PluginDesiredState::Enabled);
    assert_eq!(
        reopened_observed.package_generation,
        Some(installed_generation)
    );
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "registry.json").exists());

    // Offline exact replay must not inflate generation or re-fetch.
    server.clear_requests();
    let replay = tokio::time::timeout(
        std::time::Duration::from_secs(120),
        reopened.install_cached(
            &TrustedRegistry::new(
                "fixture",
                server.base_url(),
                &repository.root_sha256,
                None,
                home.join("state/remote-registries/fixture"),
                use_paths(&home).artifact_store(),
            )
            .unwrap(),
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            None,
        ),
    )
    .await
    .expect("Control graph replay timed out")
    .expect("Control graph replay");
    assert!(!replay.changed);
    assert!(server.requests().is_empty());
    let after_replay = reopened.observe_package("acme/root").await.unwrap();
    assert_eq!(after_replay.package_generation, Some(installed_generation));
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
    assert!(!scoped_state(&home, "extensions").exists());
}
