use super::host_support::*;
use super::*;

use a3s_use_core::{PluginHostManager, PluginOperationAction};

#[test]
fn killed_host_protocol_install_apply_replays_offline_without_reauthorization() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = expected_package_ids();
    let targets = managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target);
    let repository = TestRepository::with_targets(targets, 119, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let authorization_marker = temp.path().join("unexpected-authorization.marker");
    let apply_request_path = temp.path().join("apply-request.json");

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let apply_request = runtime.block_on(async {
        configure_host_registry(&home, &server, &repository).await;
        let host = host_manager(&home, &authorization_marker);
        plan_host_release_apply(
            &host,
            PluginOperationAction::Install,
            "1.0.0",
            "plan:host-process-install:0001",
            "apply:host-process-install:0001",
        )
        .await
        .0
    });
    std::fs::write(
        &apply_request_path,
        apply_request.canonical_bytes().unwrap(),
    )
    .unwrap();
    assert!(!authorization_marker.exists());
    drop(server);

    let pending_path = home.join("state/operations/package-graphs/install/acme/worker.json");
    let graph_path = home.join("state/package-graphs/acme/worker.json");
    let snapshot_path = home.join("state/registry.json");
    let held_lifecycle_path = managed_lifecycle_journal_path(&home, "acme/leaf-00");
    let mut interrupted = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker);
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "install", "prepared") else {
        let process_status = interrupted.try_wait().unwrap();
        let pending_bytes = file_length(&pending_path);
        let grant_summary =
            find_grant_operation(&home, "install").map(|(_, journal)| journal["phase"].clone());
        let output = terminate_child(interrupted);
        panic!(
            "Host apply did not reach prepared Grants: status={process_status:?}, pending_bytes={pending_bytes:?}, grant_phase={grant_summary:?}, child={}",
            child_output(&output)
        );
    };
    if !wait_for_lifecycle_prepare(&held_lifecycle_path) {
        let process_status = interrupted.try_wait().unwrap();
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let output = terminate_child(interrupted);
        panic!(
            "Host apply did not prepare the held dependency: status={process_status:?}, lifecycle={lifecycle:?}, grant={:?}, child={}",
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
            "Host dependency or Grant completed before the lifecycle lock was acquired: status={process_status:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let reached_cutover = wait_until(Duration::from_secs(30), || {
        let Some(snapshot) = read_json(&snapshot_path) else {
            return false;
        };
        pending_path.is_file()
            && !graph_path.exists()
            && snapshot["generation"] == 1
            && snapshot["routes"]
                .as_array()
                .is_some_and(|routes| routes.len() == DEPENDENCY_COUNT + 1)
            && route_package_ids(&snapshot) == expected_package_ids
            && snapshot["pendingCutovers"]
                .as_array()
                .is_some_and(|cutovers| cutovers.len() == 1)
            && lifecycle_is_prepared(&held_lifecycle_path)
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
            "Host apply did not reach graph-published/Grant-prepared cutover: status={process_status:?}, snapshot={snapshot:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let output = terminate_child(interrupted);
    assert!(!output.status.success(), "child unexpectedly completed");
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);
    assert!(!authorization_marker.exists());

    let grant_journal = read_json(&grant_operation_path).unwrap();
    let package_digest = grant_journal["intent"]["candidates"][0]["receipt"]["grant"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let grant_before = observe_grant(&home, &package_digest);
    assert!(matches!(grant_before, StoredWorkspaceGrant::Granted(_)));

    let recovered = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker)
        .wait_with_output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "offline Host apply recovery failed: {}",
        child_output(&recovered)
    );
    assert!(!authorization_marker.exists());
    assert_eq!(observe_grant(&home, &package_digest), grant_before);
    assert_eq!(
        grant_phase(&grant_operation_path).as_deref(),
        Some("completed")
    );
    assert_completed_lifecycles(&home);
    assert!(!pending_path.exists());
    assert!(graph_path.is_file());

    let replayed = runtime
        .block_on(host_manager(&home, &authorization_marker).apply(apply_request))
        .unwrap();
    assert!(replayed.replayed);
    assert!(!authorization_marker.exists());
    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 1);
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
    assert_eq!(route_package_ids(&snapshot), expected_package_ids);

    let history = Command::new(binary())
        .args([
            "extension",
            "diagnose",
            "acme/worker",
            "--history",
            "--scope-kind",
            "workspace",
            "--scope-id",
            MANAGED_SCOPE_ID,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(history.status.success(), "{history:?}");
    let history = json(&history);
    let history = &history["data"]["diagnostic"];
    assert_eq!(history["scope"]["kind"], "workspace");
    assert_eq!(history["scope"]["id"], MANAGED_SCOPE_ID);
    assert_eq!(history["retainedOperationCount"], 1);
    assert_eq!(history["operations"][0]["outcome"], "completed");
    assert_eq!(
        history["operations"][0]["diagnostic"]["operation"]["action"],
        "install"
    );
}
