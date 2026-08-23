use super::*;

#[test]
fn standalone_plugin_manager_cli_uses_exact_durable_plan_apply_contracts() {
    const TEST_THREAD_STACK_SIZE: usize = 8 * 1024 * 1024;

    std::thread::Builder::new()
        .name("standalone-plugin-manager-cli".to_string())
        .stack_size(TEST_THREAD_STACK_SIZE)
        .spawn(run_standalone_plugin_manager_cli_scenario)
        .unwrap()
        .join()
        .unwrap();
}

fn run_standalone_plugin_manager_cli_scenario() {
    let temporary = tempfile::tempdir().unwrap();
    let target = host_target();
    let mut targets = cognitive_tool_targets_version(
        &temporary.path().join("v1"),
        "acme/worker",
        "worker-cli",
        "1.0.0",
        &target,
    );
    targets.extend(cognitive_tool_targets_version(
        &temporary.path().join("v2"),
        "acme/worker",
        "worker-cli",
        "2.0.0",
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 89, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    // Keep the synthetic root below legacy Windows MAX_PATH after the Host
    // diagnostic store appends its two digest-bound ownership segments.
    let home = temporary.path().join("h");
    configure_registry(&server, &repository, &home, &[]);

    let searched = plugin_command(
        &home,
        &[
            "search",
            "worker",
            "--kind",
            "tool",
            "--channel",
            "stable",
            "--limit",
            "10",
        ],
    );
    assert!(searched.status.success(), "{searched:?}");
    let searched = json(&searched);
    assert_eq!(searched["data"]["totalMatches"], 2);
    assert_eq!(
        searched["data"]["plugins"][0]["record"]["packageId"],
        "acme/worker"
    );

    let inspected = plugin_command(
        &home,
        &[
            "inspect",
            "acme/worker",
            "--version",
            "1.0.0",
            "--channel",
            "stable",
        ],
    );
    assert!(inspected.status.success(), "{inspected:?}");
    assert_eq!(
        json(&inspected)["data"]["plugin"]["record"]["version"],
        "1.0.0"
    );

    server.clear_requests();
    let cached_search = plugin_command(&home, &["search", "worker", "--offline"]);
    assert!(cached_search.status.success(), "{cached_search:?}");
    assert!(server.requests().is_empty());
    let cached_inspection = plugin_command(
        &home,
        &["inspect", "acme/worker", "--version", "1.0.0", "--offline"],
    );
    assert!(cached_inspection.status.success(), "{cached_inspection:?}");
    assert!(server.requests().is_empty());

    let wrong_scope = plugin_command(
        &home,
        &[
            "list-installed",
            "--scope-kind",
            "workspace",
            "--scope-id",
            "user/current",
        ],
    );
    assert!(!wrong_scope.status.success(), "{wrong_scope:?}");
    assert_eq!(
        json(&wrong_scope)["error"]["code"],
        "use.plugin.manager_scope_mismatch"
    );

    let first_plan = plugin_command(
        &home,
        &[
            "plan-install",
            "acme/worker",
            "--registry-name",
            "fixture",
            "--version-requirement",
            "1.0.0",
            "--channel",
            "stable",
            "--surface",
            "tool/convert",
        ],
    );
    assert!(first_plan.status.success(), "{first_plan:?}");
    let first_plan = json(&first_plan);
    assert_eq!(first_plan["data"]["plan"]["plan"]["action"], "install");
    assert_eq!(first_plan["data"]["replayed"], false);
    let (install_operation_id, install_plan_digest) = plan_identity(&first_plan);

    server.clear_requests();
    let replayed_plan = plugin_command(
        &home,
        &[
            "plan-install",
            "acme/worker",
            "--registry-name",
            "fixture",
            "--version-requirement",
            "1.0.0",
            "--channel",
            "stable",
            "--surface",
            "tool/convert",
            "--offline",
        ],
    );
    assert!(replayed_plan.status.success(), "{replayed_plan:?}");
    let replayed_plan = json(&replayed_plan);
    assert_eq!(replayed_plan["data"]["replayed"], true);
    assert_eq!(
        plan_identity(&replayed_plan),
        (install_operation_id.clone(), install_plan_digest.clone())
    );
    assert!(server.requests().is_empty());

    let empty = plugin_command(&home, &["list-installed"]);
    assert!(empty.status.success(), "{empty:?}");
    assert!(json(&empty)["data"]["packages"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(extension_routes(&home).is_empty());

    let unconfirmed = plugin_command(
        &home,
        &[
            "apply-plan",
            "--operation-id",
            &install_operation_id,
            "--plan-digest",
            &install_plan_digest,
        ],
    );
    assert!(!unconfirmed.status.success(), "{unconfirmed:?}");
    assert_eq!(json(&unconfirmed)["error"]["code"], "use.cli.invalid_usage");
    assert!(extension_routes(&home).is_empty());

    let mismatched_digest = format!("sha256:{}", "f".repeat(64));
    let mismatched = plugin_command(
        &home,
        &[
            "apply-plan",
            "--operation-id",
            &install_operation_id,
            "--plan-digest",
            &mismatched_digest,
            "--yes",
        ],
    );
    assert!(!mismatched.status.success(), "{mismatched:?}");
    assert_eq!(
        json(&mismatched)["error"]["code"],
        "use.plugin.host_plan_mismatch"
    );
    assert!(extension_routes(&home).is_empty());

    server.clear_requests();
    let installed = apply_plan(&home, &install_operation_id, &install_plan_digest);
    assert!(installed.status.success(), "{installed:?}");
    let installed = json(&installed);
    assert_eq!(installed["data"]["replayed"], false);
    assert_eq!(installed["data"]["state"]["version"], "1.0.0");
    assert!(server.requests().is_empty());

    let replayed_apply = apply_plan(&home, &install_operation_id, &install_plan_digest);
    assert!(replayed_apply.status.success(), "{replayed_apply:?}");
    assert_eq!(json(&replayed_apply)["data"]["replayed"], true);
    assert!(server.requests().is_empty());

    let listed = plugin_command(&home, &["list-installed", "--limit", "1"]);
    assert!(listed.status.success(), "{listed:?}");
    let listed = json(&listed);
    assert_eq!(listed["data"]["packages"][0]["packageId"], "acme/worker");
    assert_eq!(listed["data"]["packages"][0]["state"]["version"], "1.0.0");

    let status = plugin_command(&home, &["status", "acme/worker"]);
    assert!(status.status.success(), "{status:?}");
    let status = json(&status);
    assert_eq!(status["data"]["status"]["availability"], "available");
    assert_eq!(status["data"]["status"]["state"]["desired"], "enabled");

    let upgrade_plan = plugin_command(
        &home,
        &[
            "plan-upgrade",
            "acme/worker",
            "--version-requirement",
            "2.0.0",
            "--channel",
            "stable",
            "--surface",
            "tool/convert",
        ],
    );
    assert!(upgrade_plan.status.success(), "{upgrade_plan:?}");
    let upgrade_plan = json(&upgrade_plan);
    assert_eq!(upgrade_plan["data"]["plan"]["plan"]["action"], "upgrade");
    let (upgrade_operation_id, upgrade_plan_digest) = plan_identity(&upgrade_plan);
    assert_eq!(
        json(&plugin_command(&home, &["status", "acme/worker"]))["data"]["status"]["state"]
            ["version"],
        "1.0.0"
    );

    server.clear_requests();
    let upgraded = apply_plan(&home, &upgrade_operation_id, &upgrade_plan_digest);
    assert!(upgraded.status.success(), "{upgraded:?}");
    assert_eq!(json(&upgraded)["data"]["state"]["version"], "2.0.0");
    assert!(server.requests().is_empty());

    let disable_plan = plugin_command(&home, &["plan-disable", "acme/worker"]);
    assert!(disable_plan.status.success(), "{disable_plan:?}");
    let disable_plan = json(&disable_plan);
    assert_eq!(disable_plan["data"]["status"], "planned");
    let (disable_operation_id, disable_plan_digest) = plan_identity(&disable_plan);
    let disabled = apply_plan(&home, &disable_operation_id, &disable_plan_digest);
    assert!(disabled.status.success(), "{disabled:?}");
    assert_eq!(
        json(&disabled)["data"]["state"]["desired"],
        "installed-disabled"
    );

    let no_change = plugin_command(&home, &["plan-disable", "acme/worker"]);
    assert!(no_change.status.success(), "{no_change:?}");
    let no_change = json(&no_change);
    assert_eq!(no_change["data"]["status"], "no-change");
    assert!(no_change["data"].get("plan").is_none());

    let enable_plan = plugin_command(&home, &["plan-enable", "acme/worker"]);
    assert!(enable_plan.status.success(), "{enable_plan:?}");
    let enable_plan = json(&enable_plan);
    assert_eq!(enable_plan["data"]["status"], "planned");
    let (enable_operation_id, enable_plan_digest) = plan_identity(&enable_plan);
    let enabled = apply_plan(&home, &enable_operation_id, &enable_plan_digest);
    assert!(enabled.status.success(), "{enabled:?}");
    assert_eq!(json(&enabled)["data"]["state"]["desired"], "enabled");

    let uninstall_plan = plugin_command(&home, &["plan-uninstall", "acme/worker"]);
    assert!(uninstall_plan.status.success(), "{uninstall_plan:?}");
    let uninstall_plan = json(&uninstall_plan);
    assert_eq!(
        uninstall_plan["data"]["plan"]["plan"]["action"],
        "uninstall"
    );
    let (uninstall_operation_id, uninstall_plan_digest) = plan_identity(&uninstall_plan);
    assert_eq!(extension_routes(&home), vec!["worker-cli".to_string()]);
    let uninstalled = apply_plan(&home, &uninstall_operation_id, &uninstall_plan_digest);
    assert!(uninstalled.status.success(), "{uninstalled:?}");
    assert_eq!(json(&uninstalled)["data"]["state"]["desired"], "absent");
    assert!(extension_routes(&home).is_empty());
    assert!(server.requests().is_empty());
}

fn plugin_command(home: &std::path::Path, args: &[&str]) -> Output {
    Command::new(binary())
        .arg("plugin")
        .args(args)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn apply_plan(home: &std::path::Path, operation_id: &str, plan_digest: &str) -> Output {
    plugin_command(
        home,
        &[
            "apply-plan",
            "--operation-id",
            operation_id,
            "--plan-digest",
            plan_digest,
            "--yes",
        ],
    )
}

fn plan_identity(plan: &serde_json::Value) -> (String, String) {
    (
        plan["data"]["plan"]["plan"]["operationId"]
            .as_str()
            .unwrap()
            .to_owned(),
        plan["data"]["plan"]["planDigest"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

fn extension_routes(home: &std::path::Path) -> Vec<String> {
    let snapshot = Command::new(binary())
        .args(["extension", "snapshot", "--json"])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    json(&snapshot)["data"]["registry"]["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| route["route"].as_str().unwrap().to_owned())
        .collect()
}
