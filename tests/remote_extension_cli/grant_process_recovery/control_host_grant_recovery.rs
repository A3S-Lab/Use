use super::host_support::*;
use super::*;

use a3s_use::cognitive_package::{
    CognitivePackageHostManager, StandaloneCognitivePackageLifecycleFactory,
};
use a3s_use_core::{
    PluginHostManager, PluginHostOperationObservationRequest, PluginHostOperationPhase,
    PluginOperationAction, PluginSurfaceKind, PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA,
};
use a3s_use_extension::ExtensionRegistry;

/// Control-native Host Grant recovery matrix.
///
/// Legacy kill-mid-journal checkpoints (`grants/.operations`,
/// `operations/package-graphs/`, `registry.json`, lifecycle journals) are
/// obsolete under Control. The A2 EffectsPending kill/resume boundary is
/// covered by
/// `production_effects_pending_survives_restart_and_resumes_without_generation_inflation`.
/// This suite proves Host install/upgrade/uninstall Grant transitions commit
/// only through Control, reopen without generation inflation, and never
/// reauthorize offline replay.
#[test]
fn control_host_grant_install_upgrade_uninstall_reopens_without_reauthorization() {
    std::thread::Builder::new()
        .name("control-host-grant-recovery".to_owned())
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(4)
                .enable_all()
                .build()
                .unwrap()
                .block_on(control_host_grant_recovery_scenario());
        })
        .unwrap()
        .join()
        .unwrap();
}

async fn control_host_grant_recovery_scenario() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = expected_package_ids();
    let mut targets = managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target);
    targets.extend(managed_graph_targets(
        &temp.path().join("v2"),
        "2.0.0",
        "^2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 131, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let auth_replay = temp.path().join("auth-replay.marker");
    // Authorizing hosts may or may not touch a marker depending on whether the
    // Host path still invokes authorize() for already-confirmed applies.
    let auth_ops = temp.path().join("auth-ops.marker");

    configure_host_registry(&home, &server, &repository).await;

    let install_host = authorizing_host_manager(&home, &auth_ops);
    let (install_apply, _install_lock) = plan_host_release_apply(
        &install_host,
        PluginOperationAction::Install,
        "1.0.0",
        "plan:control-host-grant:install",
        "apply:control-host-grant:install",
    )
    .await;
    assert!(install_apply
        .confirmation
        .as_ref()
        .is_some_and(|confirmation| !confirmation.plan_digest.is_empty()));
    let installed = install_host.apply(install_apply.clone()).await.unwrap();
    assert!(!installed.replayed);
    assert_control_only_authority(&home);
    assert!(installed
        .state
        .selected_surfaces
        .iter()
        .any(|surface| surface.kind == PluginSurfaceKind::Tool));
    let install_generation = installed
        .state
        .package_generation
        .expect("install must publish a package generation");
    let install_observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:control-host-grant:install".to_owned(),
        assignment_generation: install_apply.assignment_generation,
        capabilities_digest: install_apply.capabilities_digest.clone(),
        scope: install_apply.scope.clone(),
        package_id: install_apply.package_id.clone(),
        operation_id: install_apply.operation_id.clone(),
        plan_digest: install_apply.plan_digest.clone(),
    };
    let observed_install = install_host
        .observe_operation(install_observation.clone())
        .await
        .unwrap();
    assert_eq!(
        observed_install.status.phase,
        PluginHostOperationPhase::Completed
    );

    // Process reopen + offline install replay must not reauthorize or inflate.
    drop(install_host);
    server.clear_requests();
    let replay_host = host_manager(&home, &auth_replay);
    let replayed_install = replay_host.apply(install_apply.clone()).await.unwrap();
    assert!(replayed_install.replayed);
    assert!(!auth_replay.exists());
    assert!(server.requests().is_empty());
    assert_eq!(
        replayed_install.state.package_generation,
        Some(install_generation)
    );
    assert_control_only_authority(&home);
    let installed_ids = replay_host
        .installed_cognitive_package_ids()
        .await
        .unwrap()
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(installed_ids, expected_package_ids);

    // Upgrade must retire the exact Control Grant and publish the successor.
    let upgrade_host = authorizing_host_manager(&home, &auth_ops);
    let (upgrade_apply, _) = plan_host_release_apply(
        &upgrade_host,
        PluginOperationAction::Upgrade,
        "2.0.0",
        "plan:control-host-grant:upgrade",
        "apply:control-host-grant:upgrade",
    )
    .await;
    let upgraded = upgrade_host.apply(upgrade_apply.clone()).await.unwrap();
    assert!(!upgraded.replayed);
    assert!(upgraded
        .state
        .package_generation
        .is_some_and(|generation| generation > install_generation));
    assert_control_only_authority(&home);

    drop(upgrade_host);
    let upgrade_replay = host_manager(&home, &auth_replay)
        .apply(upgrade_apply)
        .await
        .unwrap();
    assert!(upgrade_replay.replayed);
    assert!(!auth_replay.exists());

    // Uninstall retires Grants through Control and leaves no legacy grant leaf.
    let uninstall_host = authorizing_host_manager(&home, &auth_ops);
    let current_lock = uninstall_host
        .installed_cognitive_package_lock(PACKAGE_ID)
        .await
        .unwrap()
        .expect("upgraded package lock must remain readable from Control");
    let uninstall_apply = plan_host_uninstall_apply(
        &uninstall_host,
        &current_lock,
        "plan:control-host-grant:uninstall",
        "apply:control-host-grant:uninstall",
    )
    .await;
    let uninstalled = uninstall_host.apply(uninstall_apply.clone()).await.unwrap();
    assert!(!uninstalled.replayed);
    assert_control_only_authority(&home);
    assert!(uninstall_host
        .installed_cognitive_package_lock(PACKAGE_ID)
        .await
        .unwrap()
        .is_none());

    drop(uninstall_host);
    let uninstall_replay = host_manager(&home, &auth_replay)
        .apply(uninstall_apply)
        .await
        .unwrap();
    assert!(uninstall_replay.replayed);
    assert!(!auth_replay.exists());
    assert_control_only_authority(&home);
}

fn authorizing_host_manager(
    home: &Path,
    authorization_marker: &Path,
) -> CognitivePackageHostManager {
    let scope = managed_host_scope();
    CognitivePackageHostManager::new(
        scope.clone(),
        HOST_BUILD_ID,
        ExtensionRegistry::new(extension_paths_for(home, scope.plan_scope())),
        Arc::new(StandaloneCognitivePackageLifecycleFactory::default()),
        Arc::new(ProcessAuthorization {
            marker: authorization_marker.to_owned(),
            allow_authorization: true,
        }),
    )
    .unwrap()
}

fn assert_control_only_authority(home: &Path) {
    let state = managed_state_root(home);
    assert!(state.join("control.sqlite3").is_file());
    assert!(!state.join("installation-snapshot.json").exists());
    assert!(!state.join("registry.json").exists());
    assert!(!state.join("extensions").exists());
    assert!(!state.join("grants").exists());
    assert!(!state.join("operations/package-graphs").exists());
}
