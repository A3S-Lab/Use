use super::*;

#[test]
fn killed_managed_install_replays_graph_and_grant_cutover_without_reauthorization() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = expected_package_ids();
    let targets = managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target);
    let repository = TestRepository::with_targets(targets, 107, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let authorization_marker = temp.path().join("authorization.marker");
    let pending_path =
        managed_state_root(&home).join("operations/package-graphs/install/acme/worker.json");
    let graph_path = managed_state_root(&home).join("installation-snapshot.json");
    let snapshot_path = managed_state_root(&home).join("registry.json");
    let held_lifecycle_path = managed_lifecycle_journal_path(&home, "acme/leaf-00");

    let mut interrupted = spawn_managed_child(ManagedChildRequest {
        home: &home,
        server: &server,
        repository: &repository,
        authorization_marker: &authorization_marker,
        action: "install",
        version: Some("1.0.0"),
        allow_authorization: true,
        offline: false,
    });
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "install", "prepared") else {
        let process_status = interrupted.try_wait().unwrap();
        let pending_bytes = file_length(&pending_path);
        let grant_summary =
            find_grant_operation(&home, "install").map(|(_, journal)| journal["phase"].clone());
        let output = terminate_child(interrupted);
        panic!(
            "managed install did not reach prepared Grants: status={process_status:?}, pending_bytes={pending_bytes:?}, grant_phase={grant_summary:?}, child={}",
            child_output(&output)
        );
    };

    if !wait_for_lifecycle_prepare(&held_lifecycle_path) {
        let process_status = interrupted.try_wait().unwrap();
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let output = terminate_child(interrupted);
        panic!(
            "managed install did not prepare the held dependency: status={process_status:?}, lifecycle={lifecycle:?}, grant={:?}, child={}",
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
            "dependency or Grant completed before the lifecycle lock was acquired: status={process_status:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
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
            "managed install did not reach graph-published/Grant-prepared cutover: status={process_status:?}, snapshot={snapshot:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let output = terminate_child(interrupted);
    assert!(!output.status.success(), "child unexpectedly completed");
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);

    let marker_before = std::fs::read(&authorization_marker).unwrap();
    let grant_journal = read_json(&grant_operation_path).unwrap();
    let package_digest = grant_journal["intent"]["candidates"][0]["receipt"]["grant"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(grant_journal["phase"], "prepared");
    let grant_before = observe_grant(&home, &package_digest);
    assert!(matches!(grant_before, StoredWorkspaceGrant::Granted(_)));
    assert!(pending_path.is_file());
    assert!(!graph_path.exists());

    let requests_before_diagnostic = server.requests().len();
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
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    let diagnostic = json(&diagnostic);
    let diagnostic = &diagnostic["data"]["diagnostic"];
    assert_eq!(diagnostic["scope"]["kind"], "workspace");
    assert_eq!(diagnostic["scope"]["id"], MANAGED_SCOPE_ID);
    assert_eq!(diagnostic["operation"]["phase"], "admitted");
    assert_eq!(diagnostic["operation"]["providerCount"], 1);
    assert_eq!(
        diagnostic["operation"]["providers"][0]["readiness"],
        "ready"
    );
    assert_eq!(diagnostic["operation"]["grant"]["required"], true);
    assert_eq!(diagnostic["operation"]["grant"]["status"], "prepared");
    assert_eq!(diagnostic["operation"]["grant"]["candidateCount"], 1);
    assert_eq!(diagnostic["operation"]["grant"]["retirementCount"], 0);
    assert_eq!(
        diagnostic["registry"]["operationCutover"]["status"],
        "recorded"
    );
    let encoded = serde_json::to_string(&diagnostic).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(authorization_marker.to_str().unwrap()));

    server.clear_requests();
    let recovered = spawn_managed_child(ManagedChildRequest {
        home: &home,
        server: &server,
        repository: &repository,
        authorization_marker: &authorization_marker,
        action: "install",
        version: Some("1.0.0"),
        allow_authorization: false,
        offline: true,
    })
    .wait_with_output()
    .unwrap();
    assert!(
        recovered.status.success(),
        "managed Grant recovery failed: {}",
        child_output(&recovered)
    );
    assert!(server.requests().is_empty());
    assert_eq!(std::fs::read(&authorization_marker).unwrap(), marker_before);
    assert_eq!(observe_grant(&home, &package_digest), grant_before);
    assert_eq!(
        grant_phase(&grant_operation_path).as_deref(),
        Some("completed")
    );
    assert_completed_lifecycles(&home);
    assert!(!pending_path.exists());
    assert!(graph_path.is_file());

    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 1);
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
    assert_eq!(route_package_ids(&snapshot), expected_package_ids);
}
