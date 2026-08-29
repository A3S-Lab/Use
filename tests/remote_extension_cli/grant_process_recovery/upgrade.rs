use super::*;

#[test]
fn killed_managed_upgrade_replays_graph_and_grant_cutover_without_reauthorization() {
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
    let repository = TestRepository::with_targets(targets, 109, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let graph_path = managed_state_root(&home).join("installation-snapshot.json");
    let snapshot_path = managed_state_root(&home).join("registry.json");

    let baseline_marker = temp.path().join("baseline-authorization.marker");
    let baseline = spawn_managed_child(ManagedChildRequest {
        home: &home,
        server: &server,
        repository: &repository,
        authorization_marker: &baseline_marker,
        action: "install",
        version: Some("1.0.0"),
        allow_authorization: true,
        offline: false,
    })
    .wait_with_output()
    .unwrap();
    assert!(
        baseline.status.success(),
        "managed baseline install failed: {}",
        child_output(&baseline)
    );
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
        panic!("baseline install did not persist its exact Grant receipt");
    };

    let authorization_marker = temp.path().join("upgrade-authorization.marker");
    let pending_path =
        managed_state_root(&home).join("operations/package-graphs/upgrade/acme/worker.json");
    let held_lifecycle_path = managed_lifecycle_journal_path(&home, "acme/leaf-00");
    let mut interrupted = spawn_managed_child(ManagedChildRequest {
        home: &home,
        server: &server,
        repository: &repository,
        authorization_marker: &authorization_marker,
        action: "upgrade",
        version: Some("2.0.0"),
        allow_authorization: true,
        offline: false,
    });
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "upgrade", "prepared") else {
        let process_status = interrupted.try_wait().unwrap();
        let pending_bytes = file_length(&pending_path);
        let grant_summary =
            find_grant_operation(&home, "upgrade").map(|(_, journal)| journal["phase"].clone());
        let output = terminate_child(interrupted);
        panic!(
            "managed upgrade did not reach prepared Grants: status={process_status:?}, pending_bytes={pending_bytes:?}, grant_phase={grant_summary:?}, child={}",
            child_output(&output)
        );
    };

    if !wait_for_lifecycle_prepare(&held_lifecycle_path) {
        let process_status = interrupted.try_wait().unwrap();
        let lifecycle = lifecycle_summary(&held_lifecycle_path);
        let output = terminate_child(interrupted);
        panic!(
            "managed upgrade did not prepare the held dependency: status={process_status:?}, lifecycle={lifecycle:?}, grant={:?}, child={}",
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
            "upgrade dependency or Grant completed before the lifecycle lock was acquired: status={process_status:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let reached_cutover = wait_until(Duration::from_secs(30), || {
        let Some(snapshot) = read_json(&snapshot_path) else {
            return false;
        };
        pending_path.is_file()
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
            "managed upgrade did not reach graph-published/Grant-prepared cutover: status={process_status:?}, snapshot={snapshot:?}, graph={graph:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let output = terminate_child(interrupted);
    assert!(!output.status.success(), "child unexpectedly completed");
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);

    let marker_before = std::fs::read(&authorization_marker).unwrap();
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

    server.clear_requests();
    let recovered = spawn_managed_child(ManagedChildRequest {
        home: &home,
        server: &server,
        repository: &repository,
        authorization_marker: &authorization_marker,
        action: "upgrade",
        version: Some("2.0.0"),
        allow_authorization: false,
        offline: true,
    })
    .wait_with_output()
    .unwrap();
    assert!(
        recovered.status.success(),
        "managed Grant upgrade recovery failed: {}",
        child_output(&recovered)
    );
    assert!(server.requests().is_empty());
    assert_eq!(std::fs::read(&authorization_marker).unwrap(), marker_before);
    assert_eq!(observe_grant(&home, &candidate_digest), candidate_grant);
    let StoredWorkspaceGrant::Revoked(revocation) = observe_grant(&home, &prior_digest) else {
        panic!("recovered upgrade did not retire the exact prior Grant");
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
