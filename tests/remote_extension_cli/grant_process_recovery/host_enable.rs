use super::host_support::*;
use super::*;

use a3s_use_core::{PluginDesiredState, PluginHostManager, PluginOperationAction};

#[test]
fn killed_host_protocol_enable_apply_replays_publication_and_grant_cutover() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = expected_package_ids();
    let targets = managed_graph_targets(&temp.path().join("v1"), "1.0.0", "^1.0.0", &target);
    let repository = TestRepository::with_targets(targets, 129, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let authorization_marker = temp.path().join("unexpected-authorization.marker");
    let apply_request_path = temp.path().join("enable-apply-request.json");
    let snapshot_path = home.join("state/registry.json");

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
        "plan:host-process-enable-baseline:0001",
        "apply:host-process-enable-baseline:0001",
    ));
    let installed = runtime.block_on(host.apply(install_apply)).unwrap();
    let disable_apply = runtime.block_on(plan_host_enablement_apply(
        &host,
        installed.state.package_generation.unwrap(),
        false,
        "plan:host-process-enable-disable:0001",
        "apply:host-process-enable-disable:0001",
    ));
    let disabled = runtime.block_on(host.apply(disable_apply)).unwrap();
    assert_eq!(
        disabled.state.desired,
        PluginDesiredState::InstalledDisabled
    );
    let disabled_state_generation = disabled.state.package_generation.unwrap();
    let disable_grant_path = find_grant_operation(&home, "disable").unwrap().0;
    let disable_grant_journal = read_json(&disable_grant_path).unwrap();
    let package_digest = disable_grant_journal["intent"]["retirements"][0]["evidence"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    let prior_revocation = observe_grant(&home, &package_digest);
    let StoredWorkspaceGrant::Revoked(prior_revocation_receipt) = &prior_revocation else {
        panic!("baseline Host disable did not retire the exact prior Grant");
    };
    let baseline_snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(baseline_snapshot["generation"], 2);
    assert_eq!(route_package_ids(&baseline_snapshot), expected_package_ids);
    assert!(!enabled_route_package_ids(&baseline_snapshot).contains(PACKAGE_ID));

    let apply_request = runtime.block_on(plan_host_enablement_apply(
        &host,
        disabled_state_generation,
        true,
        "plan:host-process-enable:0001",
        "apply:host-process-enable:0001",
    ));
    std::fs::write(
        &apply_request_path,
        apply_request.canonical_bytes().unwrap(),
    )
    .unwrap();
    drop(host);
    drop(server);
    assert!(!authorization_marker.exists());

    let lifecycle_path = managed_lifecycle_journal_path(&home, PACKAGE_ID);
    let lifecycle_lock = exclusive_lock(&lifecycle_path.with_file_name(".operation.lock"));
    let mut interrupted = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker);
    let Some(grant_operation_path) = wait_for_grant_phase(&home, "enable", "prepared") else {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let grant = find_grant_operation(&home, "enable").map(|(_, journal)| journal);
        let output = terminate_child(interrupted);
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "Host enable did not prepare Grants before lifecycle publication: status={process_status:?}, snapshot={snapshot:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    };
    let grant_store_lock = exclusive_lock(&home.join("state/grants/.store.lock"));
    if grant_phase(&grant_operation_path).as_deref() != Some("prepared") {
        let process_status = interrupted.try_wait().unwrap();
        let grant = grant_phase(&grant_operation_path);
        let output = terminate_child(interrupted);
        FileExt::unlock(&grant_store_lock).unwrap();
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "Host enable Grant advanced before the store lock was acquired: status={process_status:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);

    let reached_publication = wait_until(Duration::from_secs(30), || {
        let Some(snapshot) = read_json(&snapshot_path) else {
            return false;
        };
        let lifecycle_applied = lifecycle_summary(&lifecycle_path).is_some_and(
            |(status, completed_checkpoints, total_checkpoints)| {
                status == "applying"
                    && total_checkpoints > 0
                    && completed_checkpoints == total_checkpoints
            },
        );
        snapshot["generation"] == 3
            && route_package_ids(&snapshot) == expected_package_ids
            && snapshot["pendingCutovers"]
                .as_array()
                .is_some_and(|cutovers| cutovers.len() == 1)
            && lifecycle_applied
            && grant_phase(&grant_operation_path).as_deref() == Some("prepared")
    });
    if !reached_publication {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let lifecycle = lifecycle_summary(&lifecycle_path);
        let grant = grant_phase(&grant_operation_path);
        let output = terminate_child(interrupted);
        FileExt::unlock(&grant_store_lock).unwrap();
        panic!(
            "Host enable did not reach published/Grant-prepared cutover: status={process_status:?}, snapshot={snapshot:?}, lifecycle={lifecycle:?}, grant={grant:?}, child={}",
            child_output(&output)
        );
    }

    let output = terminate_child(interrupted);
    assert!(!output.status.success(), "child unexpectedly completed");
    FileExt::unlock(&grant_store_lock).unwrap();
    drop(grant_store_lock);
    assert!(!authorization_marker.exists());

    let grant_journal = read_json(&grant_operation_path).unwrap();
    assert_eq!(grant_journal["phase"], "prepared");
    assert!(grant_journal["intent"]["retirements"]
        .as_array()
        .is_some_and(Vec::is_empty));
    let candidate_digest = grant_journal["intent"]["candidates"][0]["receipt"]["grant"]
        ["packageDigest"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(candidate_digest, package_digest);
    let candidate_grant = observe_grant(&home, &candidate_digest);
    let StoredWorkspaceGrant::Granted(candidate_receipt) = &candidate_grant else {
        panic!("Host enable did not persist its exact candidate Grant before publication");
    };
    assert!(candidate_receipt.revision > prior_revocation_receipt.revision);

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
    assert_eq!(diagnostic["operation"]["action"], "enable");
    assert_eq!(diagnostic["operation"]["phase"], "admitted");
    assert_eq!(diagnostic["operation"]["sourceCount"], 1);
    assert_eq!(diagnostic["operation"]["providerCount"], 1);
    assert_eq!(
        diagnostic["operation"]["providers"][0]["readiness"],
        "ready"
    );
    assert_eq!(diagnostic["operation"]["download"], "not-required");
    assert_eq!(diagnostic["operation"]["grant"]["status"], "prepared");
    assert_eq!(diagnostic["operation"]["grant"]["candidateCount"], 1);
    assert_eq!(diagnostic["operation"]["grant"]["retirementCount"], 0);
    assert_eq!(
        diagnostic["registry"]["operationCutover"]["status"],
        "recorded"
    );
    assert_eq!(diagnostic["operation"]["lifecycle"][0]["action"], "enable");
    assert_eq!(
        diagnostic["operation"]["lifecycle"][0]["publication"],
        "published"
    );
    assert_eq!(
        diagnostic["operation"]["lifecycle"][0]["completedCheckpoints"],
        diagnostic["operation"]["lifecycle"][0]["totalCheckpoints"]
    );
    assert_eq!(
        diagnostic["operation"]["lifecycle"][0]["drain"],
        "not-required"
    );
    let encoded = serde_json::to_string(diagnostic).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains("idempotency"));

    let enablement_scope_digest = format!(
        "{:x}",
        Sha256::digest(format!("workspace\n{MANAGED_SCOPE_ID}").as_bytes())
    );
    let enablement_state_path = home
        .join("state/package-enablement/scopes")
        .join(enablement_scope_digest)
        .join(PACKAGE_ID)
        .join("state.json");
    let retained_enablement_state = std::fs::read(&enablement_state_path).unwrap();
    let mut corrupted_enablement_state: serde_json::Value =
        serde_json::from_slice(&retained_enablement_state).unwrap();
    corrupted_enablement_state["active"]["credential"] =
        serde_json::json!("enablement-secret-sentinel-value");
    std::fs::write(
        &enablement_state_path,
        serde_json::to_vec(&corrupted_enablement_state).unwrap(),
    )
    .unwrap();
    let invalid_diagnostic = Command::new(binary())
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
    assert!(
        !invalid_diagnostic.status.success(),
        "{invalid_diagnostic:?}"
    );
    let invalid_diagnostic = json(&invalid_diagnostic);
    assert_eq!(
        invalid_diagnostic["error"]["code"],
        "use.plugin.operation_diagnostic_state_invalid"
    );
    let invalid_diagnostic = serde_json::to_string(&invalid_diagnostic).unwrap();
    assert!(!invalid_diagnostic.contains("enablement-secret-sentinel-value"));
    assert!(!invalid_diagnostic.contains(home.to_str().unwrap()));
    std::fs::write(&enablement_state_path, retained_enablement_state).unwrap();

    let recovered = spawn_host_apply_child(&home, &apply_request_path, &authorization_marker)
        .wait_with_output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "Host enable recovery failed: {}",
        child_output(&recovered)
    );
    assert!(!authorization_marker.exists());
    assert_eq!(observe_grant(&home, &candidate_digest), candidate_grant);
    assert_eq!(
        grant_phase(&grant_operation_path).as_deref(),
        Some("completed")
    );
    assert_completed_lifecycles(&home);

    let replayed = runtime
        .block_on(host_manager(&home, &authorization_marker).apply(apply_request))
        .unwrap();
    assert!(replayed.replayed);
    assert_eq!(replayed.state.desired, PluginDesiredState::Enabled);
    assert!(replayed.state.package_generation.unwrap() > disabled_state_generation);
    assert!(!authorization_marker.exists());
    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 3);
    assert_eq!(route_package_ids(&snapshot), expected_package_ids);
    assert_eq!(enabled_route_package_ids(&snapshot), expected_package_ids);
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
}
