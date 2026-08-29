use super::host_support::*;
use super::*;

use a3s_use_core::{
    PluginDesiredState, PluginHostManager, PluginObservedState, PluginOperationAction,
};

#[test]
fn killed_host_protocol_uninstall_apply_replays_without_reauthorization() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = expected_package_ids();
    let targets = managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target);
    let repository = TestRepository::with_targets(targets, 125, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let authorization_marker = temp.path().join("unexpected-authorization.marker");
    let apply_request_path = temp.path().join("uninstall-apply-request.json");
    let graph_path = managed_state_root(&home).join("installation-snapshot.json");
    let snapshot_path = managed_state_root(&home).join("registry.json");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(configure_host_registry(&home, &server, &repository));
    let host = host_manager(&home, &authorization_marker);
    let (baseline_apply, baseline_lock) = runtime.block_on(plan_host_release_apply(
        &host,
        PluginOperationAction::Install,
        "1.0.0",
        "plan:host-process-uninstall-baseline:0001",
        "apply:host-process-uninstall-baseline:0001",
    ));
    let installed = runtime.block_on(host.apply(baseline_apply)).unwrap();
    assert_eq!(installed.state.version.as_deref(), Some("1.0.0"));
    assert!(!authorization_marker.exists());

    let baseline_grant_path = find_grant_operation(&home, "install").unwrap().0;
    let baseline_grant_journal = read_json(&baseline_grant_path).unwrap();
    let prior_digest = baseline_grant_journal["intent"]["candidates"][0]["receipt"]["grant"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let prior_grant = observe_grant(&home, &prior_digest);
    let StoredWorkspaceGrant::Granted(prior_receipt) = &prior_grant else {
        panic!("baseline Host install did not persist its exact Grant receipt");
    };
    let baseline_snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(baseline_snapshot["generation"], 1);
    assert_eq!(route_package_ids(&baseline_snapshot), expected_package_ids);

    let apply_request = runtime.block_on(plan_host_uninstall_apply(
        &host,
        &baseline_lock,
        "plan:host-process-uninstall:0001",
        "apply:host-process-uninstall:0001",
    ));
    std::fs::write(
        &apply_request_path,
        apply_request.canonical_bytes().unwrap(),
    )
    .unwrap();
    drop(host);
    drop(server);

    let pending_path =
        managed_state_root(&home).join("operations/package-graphs/uninstall/acme/worker.json");
    let held_lifecycle_path = managed_lifecycle_journal_path(&home, "acme/leaf-00");
    let lifecycle_lock = exclusive_lock(&held_lifecycle_path.with_file_name(".operation.lock"));
    let mut interrupted = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker);
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "uninstall", "prepared") else {
        let process_status = interrupted.try_wait().unwrap();
        let pending_bytes = file_length(&pending_path);
        let grant_summary =
            find_grant_operation(&home, "uninstall").map(|(_, journal)| journal["phase"].clone());
        let output = terminate_child(interrupted);
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "Host uninstall did not reach prepared Grants: status={process_status:?}, pending_bytes={pending_bytes:?}, grant_phase={grant_summary:?}, child={}",
            child_output(&output)
        );
    };

    let reached_cutover = wait_until(Duration::from_secs(30), || {
        let Some(snapshot) = read_json(&snapshot_path) else {
            return false;
        };
        pending_path.is_file()
            && graph_path.is_file()
            && snapshot["generation"] == 2
            && snapshot["routes"].as_array().is_some_and(Vec::is_empty)
            && snapshot["pendingCutovers"]
                .as_array()
                .is_some_and(|cutovers| cutovers.len() == 1)
            && grant_phase(&grant_operation_path).as_deref() == Some("prepared")
    });
    if !reached_cutover {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let grant = grant_phase(&grant_operation_path);
        let output = terminate_child(interrupted);
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "Host uninstall did not reach graph-hidden/Grant-prepared cutover: status={process_status:?}, snapshot={snapshot:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let output = terminate_child(interrupted);
    assert!(!output.status.success(), "child unexpectedly completed");
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);
    assert!(!authorization_marker.exists());

    let grant_journal = read_json(&grant_operation_path).unwrap();
    assert_eq!(grant_journal["phase"], "prepared");
    assert!(grant_journal["intent"]["candidates"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert_eq!(
        grant_journal["intent"]["retirements"][0]["evidence"]["packageDigest"],
        prior_digest
    );
    assert_eq!(observe_grant(&home, &prior_digest), prior_grant);

    let recovered = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker)
        .wait_with_output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "Host uninstall recovery failed: {}",
        child_output(&recovered)
    );
    assert!(!authorization_marker.exists());
    let StoredWorkspaceGrant::Revoked(revocation) = observe_grant(&home, &prior_digest) else {
        panic!("recovered Host uninstall did not retire the exact prior Grant");
    };
    assert_eq!(revocation.package_digest, prior_digest);
    assert_eq!(revocation.prior_revision, prior_receipt.revision);
    assert_eq!(revocation.prior_grant_digest, prior_receipt.grant_digest);
    assert_eq!(
        grant_phase(&grant_operation_path).as_deref(),
        Some("completed")
    );
    assert_completed_lifecycles(&home);
    assert!(!pending_path.exists());
    let installation_snapshot = read_json(&graph_path).unwrap();
    assert_eq!(installation_snapshot["generation"], 2);
    assert!(installation_snapshot["roots"]
        .as_array()
        .is_some_and(Vec::is_empty));
    assert!(installation_snapshot["packages"]
        .as_array()
        .is_some_and(Vec::is_empty));
    for package_id in &expected_package_ids {
        assert!(!managed_state_root(&home)
            .join("extensions")
            .join(format!("{package_id}.json"))
            .exists());
    }

    let replayed = runtime
        .block_on(host_manager(&home, &authorization_marker).apply(apply_request))
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.state.desired, PluginDesiredState::Absent);
    assert_eq!(replayed.state.observed, PluginObservedState::Removed);
    assert!(replayed.state.version.is_none());
    assert!(replayed.state.selected_surfaces.is_empty());
    assert!(!authorization_marker.exists());
    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 2);
    assert!(snapshot["routes"].as_array().is_some_and(Vec::is_empty));
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
}
