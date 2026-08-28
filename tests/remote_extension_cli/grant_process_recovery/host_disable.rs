use super::host_support::*;
use super::*;

use a3s_use_core::{PluginDesiredState, PluginHostManager, PluginOperationAction};

#[test]
fn killed_host_protocol_disable_apply_replays_hide_drain_and_grant_retirement() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_packages = expected_package_ids();
    let mut expected_enabled_packages = expected_packages.clone();
    expected_enabled_packages.remove(PACKAGE_ID);
    let targets = managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target);
    let repository = TestRepository::with_targets(targets, 127, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let authorization_marker = temp.path().join("unexpected-authorization.marker");
    let apply_request_path = temp.path().join("disable-apply-request.json");
    let snapshot_path = managed_state_root(&home).join("registry.json");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(configure_host_registry(&home, &server, &repository));
    let host = host_manager(&home, &authorization_marker);
    let (install_apply, _) = runtime.block_on(plan_host_release_apply(
        &host,
        PluginOperationAction::Install,
        "1.0.0",
        "plan:host-process-disable-baseline:0001",
        "apply:host-process-disable-baseline:0001",
    ));
    let installed = runtime.block_on(host.apply(install_apply)).unwrap();
    let installed_state_generation = installed.state.package_generation.unwrap();
    assert_eq!(installed.state.desired, PluginDesiredState::Enabled);

    let extension_registry = ExtensionRegistry::new(extension_paths_for(
        &home,
        managed_host_scope().plan_scope(),
    ));
    let installed_extension = runtime
        .block_on(extension_registry.get(PACKAGE_ID))
        .unwrap()
        .unwrap();
    let lifecycle_generation = installed_extension.receipt.lifecycle_generation.unwrap();
    let install_grant_path = find_grant_operation(&home, "install").unwrap().0;
    let install_grant_journal = read_json(&install_grant_path).unwrap();
    let prior_digest = install_grant_journal["intent"]["candidates"][0]["receipt"]["grant"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let prior_grant = observe_grant(&home, &prior_digest);
    let StoredWorkspaceGrant::Granted(prior_receipt) = &prior_grant else {
        panic!("baseline Host install did not persist its exact Grant receipt");
    };

    let apply_request = runtime.block_on(plan_host_enablement_apply(
        &host,
        installed_state_generation,
        false,
        "plan:host-process-disable:0001",
        "apply:host-process-disable:0001",
    ));
    std::fs::write(
        &apply_request_path,
        apply_request.canonical_bytes().unwrap(),
    )
    .unwrap();
    drop(host);
    drop(server);
    assert!(!authorization_marker.exists());

    let route_lock = exclusive_lock(
        &managed_state_root(&home)
            .join("route-locks/acme/worker")
            .join(format!("{lifecycle_generation:020}.lock")),
    );
    let mut interrupted = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker);
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "disable", "cutover-committed")
    else {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let grant = find_grant_operation(&home, "disable").map(|(_, journal)| journal);
        let output = terminate_child(interrupted);
        FileExt::unlock(&route_lock).unwrap();
        panic!(
            "Host disable did not reach Grant cutover before drain: status={process_status:?}, snapshot={snapshot:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    };
    let reached_cutover_drain = wait_until(Duration::from_secs(30), || {
        let Some(snapshot) = read_json(&snapshot_path) else {
            return false;
        };
        snapshot["generation"] == 2
            && route_package_ids(&snapshot) == expected_packages
            && enabled_route_package_ids(&snapshot) == expected_enabled_packages
            && snapshot["pendingCutovers"]
                .as_array()
                .is_some_and(|cutovers| cutovers.len() == 1)
            && grant_phase(&grant_operation_path).as_deref() == Some("cutover-committed")
    });
    if !reached_cutover_drain {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let lifecycle = lifecycle_summary(&managed_lifecycle_journal_path(&home, PACKAGE_ID));
        let grant = grant_phase(&grant_operation_path);
        let output = terminate_child(interrupted);
        FileExt::unlock(&route_lock).unwrap();
        panic!(
            "Host disable did not reach hidden/draining cutover: status={process_status:?}, snapshot={snapshot:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let output = terminate_child(interrupted);
    assert!(!output.status.success(), "child unexpectedly completed");
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);
    assert!(!authorization_marker.exists());
    assert_eq!(observe_grant(&home, &prior_digest), prior_grant);
    let grant_journal = read_json(&grant_operation_path).unwrap();
    assert!(grant_journal["intent"]["candidates"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert_eq!(
        grant_journal["intent"]["retirements"][0]["evidence"]["packageDigest"],
        prior_digest
    );

    let diagnostic = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            PACKAGE_ID,
            "--scope-kind",
            "workspace",
            "--scope-id",
            MANAGED_SCOPE_ID,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(diagnostic.status.success(), "{diagnostic:?}");
    let diagnostic = json(&diagnostic);
    let diagnostic = &diagnostic["data"]["diagnostic"];
    assert_eq!(diagnostic["operation"]["action"], "disable");
    assert_eq!(diagnostic["operation"]["phase"], "admitted");
    assert_eq!(diagnostic["operation"]["sourceCount"], 1);
    assert_eq!(diagnostic["operation"]["providerCount"], 0);
    assert_eq!(diagnostic["operation"]["download"], "not-required");
    assert_eq!(
        diagnostic["operation"]["grant"]["status"],
        "cutover-committed"
    );
    assert_eq!(diagnostic["operation"]["grant"]["candidateCount"], 0);
    assert_eq!(diagnostic["operation"]["grant"]["retirementCount"], 1);
    assert_eq!(
        diagnostic["registry"]["operationCutover"]["status"],
        "recorded"
    );
    assert_eq!(diagnostic["operation"]["lifecycle"][0]["action"], "disable");
    assert_eq!(
        diagnostic["operation"]["lifecycle"][0]["publication"],
        "hidden"
    );
    assert_eq!(diagnostic["operation"]["lifecycle"][0]["drain"], "pending");
    assert_eq!(
        diagnostic["operation"]["lifecycle"][0]["currentCheckpoint"]["kind"],
        "calls-drained"
    );
    let encoded = serde_json::to_string(diagnostic).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains("idempotency"));

    let recovered = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker)
        .wait_with_output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "Host disable recovery failed: {}",
        child_output(&recovered)
    );
    assert!(!authorization_marker.exists());
    let StoredWorkspaceGrant::Revoked(revocation) = observe_grant(&home, &prior_digest) else {
        panic!("recovered Host disable did not retire the exact prior Grant");
    };
    assert_eq!(revocation.package_digest, prior_digest);
    assert_eq!(revocation.prior_revision, prior_receipt.revision);
    assert_eq!(revocation.prior_grant_digest, prior_receipt.grant_digest);
    assert_eq!(
        grant_phase(&grant_operation_path).as_deref(),
        Some("completed")
    );
    assert_completed_lifecycles(&home);

    let replayed = runtime
        .block_on(host_manager(&home, &authorization_marker).apply(apply_request))
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(
        replayed.state.desired,
        PluginDesiredState::InstalledDisabled
    );
    assert!(replayed.state.package_generation.unwrap() > installed_state_generation);
    assert!(!authorization_marker.exists());
    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 2);
    assert_eq!(route_package_ids(&snapshot), expected_packages);
    assert_eq!(
        enabled_route_package_ids(&snapshot),
        expected_enabled_packages
    );
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
}
