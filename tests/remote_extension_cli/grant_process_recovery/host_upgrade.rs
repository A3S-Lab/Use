use super::host_support::*;
use super::*;

use a3s_use_core::{PluginHostManager, PluginOperationAction};

#[test]
fn killed_host_protocol_upgrade_apply_replays_offline_without_reauthorization() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = expected_package_ids();
    let repository = TestRepository::with_targets(
        managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target),
        121,
        FUTURE,
    );
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let authorization_marker = temp.path().join("unexpected-authorization.marker");
    let apply_request_path = temp.path().join("upgrade-apply-request.json");
    let graph_path = managed_state_root(&home).join("installation-snapshot.json");
    let snapshot_path = managed_state_root(&home).join("registry.json");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(configure_host_registry(&home, &server, &repository));
    let host = host_manager(&home, &authorization_marker);
    let (baseline_apply, _) = runtime.block_on(plan_host_release_apply(
        &host,
        PluginOperationAction::Install,
        "1.0.0",
        "plan:host-process-upgrade-baseline:0001",
        "apply:host-process-upgrade-baseline:0001",
    ));
    let installed = runtime.block_on(host.apply(baseline_apply)).unwrap();
    assert_eq!(installed.state.version.as_deref(), Some("1.0.0"));
    assert!(!authorization_marker.exists());

    let baseline_graph = read_json(&graph_path).unwrap();
    assert!(graph_package_versions(&baseline_graph)
        .values()
        .all(|version| version == "1.0.0"));
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

    let mut updated_targets =
        managed_graph_targets(&temp.path().join("updated-v1"), "1.0.0", "^1.0.0", &target);
    updated_targets.extend(managed_graph_targets(
        &temp.path().join("v2"),
        "2.0.0",
        "^2.0.0",
        &target,
    ));
    let updated_repository = TestRepository::with_targets(updated_targets, 122, FUTURE);
    assert_eq!(updated_repository.root_sha256, repository.root_sha256);
    server.replace_routes(updated_repository.routes.clone());
    let (apply_request, _) = runtime.block_on(plan_host_release_apply(
        &host,
        PluginOperationAction::Upgrade,
        "2.0.0",
        "plan:host-process-upgrade:0001",
        "apply:host-process-upgrade:0001",
    ));
    std::fs::write(
        &apply_request_path,
        apply_request.canonical_bytes().unwrap(),
    )
    .unwrap();
    drop(host);
    drop(server);

    let pending_path =
        managed_state_root(&home).join("operations/package-graphs/upgrade/acme/worker.json");
    let held_lifecycle_path = managed_lifecycle_journal_path(&home, "acme/leaf-00");
    let mut interrupted = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker);
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "upgrade", "prepared") else {
        let process_status = interrupted.try_wait().unwrap();
        let pending_bytes = file_length(&pending_path);
        let grant_summary =
            find_grant_operation(&home, "upgrade").map(|(_, journal)| journal["phase"].clone());
        let output = terminate_child(interrupted);
        panic!(
            "Host upgrade did not reach prepared Grants: status={process_status:?}, pending_bytes={pending_bytes:?}, grant_phase={grant_summary:?}, child={}",
            child_output(&output)
        );
    };
    if !wait_for_lifecycle_prepare(&held_lifecycle_path) {
        let process_status = interrupted.try_wait().unwrap();
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let output = terminate_child(interrupted);
        panic!(
            "Host upgrade did not prepare the held dependency: status={process_status:?}, lifecycle={lifecycle:?}, grant={:?}, child={}",
            grant_phase(&grant_operation_path),
            child_output(&output)
        );
    }
    let lifecycle_lock = exclusive_lock(&held_lifecycle_path.with_file_name(".operation.lock"));
    if !lifecycle_is_prepared(&held_lifecycle_path)
        || grant_phase(&grant_operation_path).as_deref() != Some("prepared")
    {
        let process_status = interrupted.try_wait().unwrap();
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let grant = grant_phase(&grant_operation_path);
        let output = terminate_child(interrupted);
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "Host upgrade dependency or Grant completed before the lifecycle lock was acquired: status={process_status:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let reached_cutover = wait_until(Duration::from_secs(30), || {
        let Some(snapshot) = read_json(&snapshot_path) else {
            return false;
        };
        pending_path.is_file()
            && graph_path.is_file()
            && snapshot["generation"] == 2
            && snapshot["packages"]
                .as_array()
                .is_some_and(|packages| packages.len() == DEPENDENCY_COUNT + 1)
            && published_package_ids(&snapshot) == expected_package_ids
            && snapshot["pendingCutovers"]
                .as_array()
                .is_some_and(|cutovers| cutovers.len() == 1)
            && graph_package_versions(&read_json(&graph_path).unwrap())
                .values()
                .all(|version| version == "1.0.0")
            && lifecycle_is_prepared(&held_lifecycle_path)
            && grant_phase(&grant_operation_path).as_deref() == Some("prepared")
    });
    if !reached_cutover {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let graph = read_json(&graph_path).map(|graph| graph_package_versions(&graph));
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let grant = grant_phase(&grant_operation_path);
        let output = terminate_child(interrupted);
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "Host upgrade did not reach graph-published/Grant-prepared cutover: status={process_status:?}, snapshot={snapshot:?}, graph={graph:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
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
    assert_eq!(
        grant_journal["intent"]["retirements"][0]["evidence"]["packageDigest"],
        prior_digest
    );
    let candidate_digest = grant_journal["intent"]["candidates"][0]["receipt"]["grant"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let candidate_grant = observe_grant(&home, &candidate_digest);
    assert!(matches!(candidate_grant, StoredWorkspaceGrant::Granted(_)));
    assert_eq!(observe_grant(&home, &prior_digest), prior_grant);

    let recovered = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker)
        .wait_with_output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "offline Host upgrade recovery failed: {}",
        child_output(&recovered)
    );
    assert!(!authorization_marker.exists());
    assert_eq!(observe_grant(&home, &candidate_digest), candidate_grant);
    let StoredWorkspaceGrant::Revoked(revocation) = observe_grant(&home, &prior_digest) else {
        panic!("recovered Host upgrade did not retire the exact prior Grant");
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

    let replayed = runtime
        .block_on(host_manager(&home, &authorization_marker).apply(apply_request))
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.state.version.as_deref(), Some("2.0.0"));
    assert!(!authorization_marker.exists());
    let graph = read_json(&graph_path).unwrap();
    assert!(graph_package_versions(&graph)
        .values()
        .all(|version| version == "2.0.0"));
    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 2);
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
    assert_eq!(published_package_ids(&snapshot), expected_package_ids);
}
