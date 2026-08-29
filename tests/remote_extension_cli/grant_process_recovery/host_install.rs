use super::host_support::*;
use super::*;

use a3s_use_core::{
    PluginHostManager, PluginHostOperationObservationRequest, PluginHostOperationPhase,
    PluginHostOperationWatchRequest, PluginOperationAction,
    PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA, PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA,
};

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
    let observation = PluginHostOperationObservationRequest {
        schema: PLUGIN_HOST_OPERATION_OBSERVATION_REQUEST_SCHEMA.to_owned(),
        request_id: "observe:host-process-install:0001".to_owned(),
        assignment_generation: apply_request.assignment_generation,
        capabilities_digest: apply_request.capabilities_digest.clone(),
        scope: apply_request.scope.clone(),
        package_id: apply_request.package_id.clone(),
        operation_id: apply_request.operation_id.clone(),
        plan_digest: apply_request.plan_digest.clone(),
    };
    let initial_observation = runtime
        .block_on(host_manager(&home, &authorization_marker).observe_operation(observation.clone()))
        .unwrap();
    assert_eq!(
        initial_observation.status.phase,
        PluginHostOperationPhase::AwaitingConfirmation
    );
    std::fs::write(
        &apply_request_path,
        apply_request.canonical_bytes().unwrap(),
    )
    .unwrap();
    assert!(!authorization_marker.exists());
    let registry_url = server.base_url().to_owned();
    drop(server);

    let pending_path =
        managed_state_root(&home).join("operations/package-graphs/install/acme/worker.json");
    let graph_path = managed_state_root(&home).join("installation-snapshot.json");
    let snapshot_path = managed_state_root(&home).join("registry.json");
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

    let final_observation = runtime
        .block_on(host_manager(&home, &authorization_marker).watch_operation(
            PluginHostOperationWatchRequest {
                schema: PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA.to_owned(),
                observation: observation.clone(),
                after_revision: Some(initial_observation.revision),
                timeout_ms: 0,
            },
        ))
        .unwrap();
    assert!(final_observation.changed);
    assert!(!final_observation.timed_out);
    assert_eq!(
        final_observation.status.phase,
        PluginHostOperationPhase::Completed
    );
    let terminal_replay = runtime
        .block_on(host_manager(&home, &authorization_marker).watch_operation(
            PluginHostOperationWatchRequest {
                schema: PLUGIN_HOST_OPERATION_WATCH_REQUEST_SCHEMA.to_owned(),
                observation,
                after_revision: Some(final_observation.revision.clone()),
                timeout_ms: 0,
            },
        ))
        .unwrap();
    assert!(!terminal_replay.changed);
    assert!(terminal_replay.timed_out);
    assert_eq!(terminal_replay.revision, final_observation.revision);

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
    let encoded = serde_json::to_string(history).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(&registry_url));
    assert!(!encoded.contains("plan:host-process-install:0001"));
    assert!(!encoded.contains("apply:host-process-install:0001"));
    assert!(!encoded.contains("package-diagnostic-history"));
    assert!(!encoded.contains("idempotency"));
}
