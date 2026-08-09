use super::*;

#[test]
fn schema_v3_uninstall_rejects_recovery_when_exact_graph_evidence_was_deleted() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let base = cognitive_skill_target(temp.path(), "acme/base", "base", Vec::new(), &target);
    let root = cognitive_skill_target(
        temp.path(),
        "acme/root",
        "root",
        vec![PluginPackageDependency::new("acme/base", "^1.0.0").unwrap()],
        &target,
    );
    let repository = TestRepository::with_targets(vec![root, base], 29, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let root_receipt = home.join("state/extensions/acme/root.json");
    let base_receipt = home.join("state/extensions/acme/base.json");
    let pending_path = home.join("state/operations/package-graphs/uninstall/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");
    assert!(root_receipt.exists());
    assert!(base_receipt.exists());
    std::fs::remove_file(&graph_path).unwrap();

    let rejected = cognitive_uninstall(&home, "acme/root");
    assert!(!rejected.status.success(), "{rejected:?}");
    assert_eq!(
        json(&rejected)["error"]["code"],
        "use.plugin.package_graph_missing"
    );
    assert!(root_receipt.exists());
    assert!(base_receipt.exists());
    assert!(!graph_path.exists());
    assert!(!pending_path.exists());
}

#[test]
fn schema_v3_uninstall_rejects_missing_generation_without_durable_cutover() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target_version(
        temp.path(),
        "acme/root",
        "root",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![package], 73, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let selected_receipt = home.join("state/extensions/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");
    let pending_path = home.join("state/operations/package-graphs/uninstall/acme/root.json");
    let snapshot_path = home.join("state/registry.json");
    let registry_lock = exclusive_lock(&home.join("state/extensions/.registry.lock"));
    let interrupted = cognitive_uninstall(&home, "acme/root");
    assert!(!interrupted.status.success(), "{interrupted:?}");
    assert_eq!(json(&interrupted)["error"]["code"], "use.extension.busy");
    FileExt::unlock(&registry_lock).unwrap();
    drop(registry_lock);
    assert!(pending_path.exists());

    let graph_before = std::fs::read(&graph_path).unwrap();
    let pending_before = std::fs::read(&pending_path).unwrap();
    let snapshot_before = std::fs::read(&snapshot_path).unwrap();
    std::fs::remove_file(&selected_receipt).unwrap();

    let rejected = cognitive_uninstall(&home, "acme/root");
    assert!(!rejected.status.success(), "{rejected:?}");
    assert_eq!(
        json(&rejected)["error"]["code"],
        "use.extension.lifecycle_package_graph_invalid"
    );
    assert_eq!(std::fs::read(&graph_path).unwrap(), graph_before);
    assert_eq!(std::fs::read(&pending_path).unwrap(), pending_before);
    assert_eq!(std::fs::read(&snapshot_path).unwrap(), snapshot_before);
}

#[cfg(any(unix, windows))]
#[test]
fn schema_v3_upgrade_replays_removed_node_cleanup_without_generation_inflation() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_skill_target_version(
        &temp.path().join("first"),
        "acme/root",
        "root",
        "1.0.0",
        vec![PluginPackageDependency::new("acme/obsolete", "^1.0.0").unwrap()],
        &target,
    );
    let next = cognitive_skill_target_version(
        &temp.path().join("next"),
        "acme/root",
        "root",
        "1.1.0",
        Vec::new(),
        &target,
    );
    let obsolete = cognitive_skill_target_version(
        &temp.path().join("obsolete"),
        "acme/obsolete",
        "obsolete",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, next, obsolete], 67, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let snapshot_path = home.join("state/registry.json");
    let obsolete_receipt = home.join("state/extensions/acme/obsolete.json");
    let pending_path = home.join("state/operations/package-graphs/upgrade/acme/root.json");
    let snapshot_before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    let obsolete_installed: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&obsolete_receipt).unwrap()).unwrap();
    let obsolete_generation = obsolete_installed["lifecycleGeneration"].as_u64().unwrap();
    let obsolete_sha256 = obsolete_installed["packageSha256"].as_str().unwrap();
    let obsolete_retained_receipt = home
        .join("state/extension-generations/acme/obsolete")
        .join(format!("{obsolete_generation:020}-{obsolete_sha256}.json"));
    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks/acme/obsolete")
            .join(format!("{obsolete_generation:020}.lock")),
    );

    let mut interrupted = Command::new(binary())
        .args([
            "upgrade",
            "acme/root",
            "--registry-name",
            "fixture",
            "--version",
            "1.1.0",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_removed_drain = wait_until(Duration::from_secs(15), || {
        if !pending_path.exists()
            || obsolete_receipt.exists()
            || !obsolete_retained_receipt.exists()
        {
            return false;
        }
        let Ok(snapshot) = std::fs::read(&snapshot_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .ok_or(())
        else {
            return false;
        };
        let Ok(receipt) = std::fs::read(&obsolete_retained_receipt)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .ok_or(())
        else {
            return false;
        };
        receipt["enabled"] == false
            && snapshot["routes"].as_array().is_some_and(|routes| {
                routes
                    .iter()
                    .all(|route| route["packageId"] != "acme/obsolete")
                    && routes.iter().any(|route| route["packageId"] == "acme/root")
            })
    });
    if !reached_removed_drain {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = std::fs::read_to_string(&snapshot_path).ok();
        let selected_receipt = std::fs::read_to_string(&obsolete_receipt).ok();
        let retained_receipt = std::fs::read_to_string(&obsolete_retained_receipt).ok();
        let pending = std::fs::read_to_string(&pending_path).ok();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        FileExt::unlock(&route_lock).unwrap();
        panic!(
            "upgrade did not reach the removed dependency drain checkpoint: status={process_status:?}, snapshot={snapshot:?}, selected_receipt={selected_receipt:?}, retained_receipt={retained_receipt:?}, pending={pending:?}"
        );
    }
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);

    let generation_after_cutover =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&snapshot_path).unwrap())
            .unwrap()["generation"]
            .as_u64()
            .unwrap();
    assert_eq!(
        generation_after_cutover,
        snapshot_before["generation"].as_u64().unwrap() + 1
    );
    assert!(pending_path.exists());

    let recovered =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/root", "1.1.0", &[]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert_eq!(
        json(&recovered)["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/obsolete"])
    );
    assert!(!obsolete_receipt.exists());
    assert!(!obsolete_retained_receipt.exists());
    assert!(!pending_path.exists());
    let generation_after_replay =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&snapshot_path).unwrap())
            .unwrap()["generation"]
            .as_u64()
            .unwrap();
    assert_eq!(generation_after_replay, generation_after_cutover);
}

#[cfg(any(unix, windows))]
#[test]
fn schema_v3_uninstall_replays_cutover_drain_and_removal_after_real_process_kill() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target_version(
        temp.path(),
        "acme/root",
        "root",
        "1.0.0",
        Vec::new(),
        &target,
    );
    let repository = TestRepository::with_targets(vec![package], 71, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let selected_receipt = home.join("state/extensions/acme/root.json");
    let installed_receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&selected_receipt).unwrap()).unwrap();
    let lifecycle_generation = installed_receipt["lifecycleGeneration"].as_u64().unwrap();
    let package_sha256 = installed_receipt["packageSha256"].as_str().unwrap();
    let retained_receipt = home
        .join("state/extension-generations/acme/root")
        .join(format!("{lifecycle_generation:020}-{package_sha256}.json"));
    let pending_path = home.join("state/operations/package-graphs/uninstall/acme/root.json");
    let snapshot_path = home.join("state/registry.json");
    let snapshot_before: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();

    let lifecycle_path = lifecycle_journal_path(&home, "acme/root");
    let lifecycle_lock = exclusive_lock(&lifecycle_path.with_file_name(".operation.lock"));
    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks/acme/root")
            .join(format!("{lifecycle_generation:020}.lock")),
    );
    let mut interrupted = Command::new(binary())
        .args(["uninstall", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();

    let reached_cutover_before_receipt = wait_until(Duration::from_secs(15), || {
        let retained_disabled = std::fs::read(&retained_receipt)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|receipt| receipt["enabled"] == false);
        let hidden_with_pending_cutover = std::fs::read(&snapshot_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|snapshot| {
                snapshot["pendingCutovers"]
                    .as_array()
                    .is_some_and(|cutovers| cutovers.len() == 1)
                    && snapshot["routes"].as_array().is_some_and(|routes| {
                        routes.iter().all(|route| route["packageId"] != "acme/root")
                    })
            });
        pending_path.exists()
            && !selected_receipt.exists()
            && retained_disabled
            && hidden_with_pending_cutover
    });
    if !reached_cutover_before_receipt {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = std::fs::read_to_string(&snapshot_path).ok();
        let selected = std::fs::read_to_string(&selected_receipt).ok();
        let retained = std::fs::read_to_string(&retained_receipt).ok();
        let pending = std::fs::read_to_string(&pending_path).ok();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        FileExt::unlock(&lifecycle_lock).unwrap();
        FileExt::unlock(&route_lock).unwrap();
        panic!(
            "uninstall did not reach the cutover-before-receipt checkpoint: status={process_status:?}, snapshot={snapshot:?}, selected={selected:?}, retained={retained:?}, pending={pending:?}"
        );
    }

    let pending: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&pending_path).unwrap()).unwrap();
    let operation_id = pending["envelope"]["plan"]["operationId"]
        .as_str()
        .unwrap()
        .to_owned();
    let lifecycle_before_recovery: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lifecycle_path).unwrap()).unwrap();
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);
    assert_eq!(lifecycle_before_recovery["intent"]["action"], "install");
    assert_eq!(lifecycle_before_recovery["status"], "completed");

    let generation_after_cutover =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&snapshot_path).unwrap())
            .unwrap()["generation"]
            .as_u64()
            .unwrap();
    assert_eq!(
        generation_after_cutover,
        snapshot_before["generation"].as_u64().unwrap() + 1
    );
    assert!(pending_path.exists());

    let mut recovery = Command::new(binary())
        .args(["uninstall", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let reached_recovery_drain = wait_until(Duration::from_secs(15), || {
        std::fs::read(&lifecycle_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|lifecycle| {
                let completed = lifecycle["receipts"].as_array().map_or(0, Vec::len);
                lifecycle["intent"]["action"] == "uninstall"
                    && lifecycle["status"] == "applying"
                    && lifecycle["intent"]["checkpoints"]
                        .as_array()
                        .and_then(|checkpoints| checkpoints.get(completed))
                        .is_some_and(|checkpoint| checkpoint["kind"] == "calls-drained")
            })
    });
    if !reached_recovery_drain {
        let process_status = recovery.try_wait().unwrap();
        let lifecycle = std::fs::read_to_string(&lifecycle_path).ok();
        let snapshot = std::fs::read_to_string(&snapshot_path).ok();
        let retained = std::fs::read_to_string(&retained_receipt).ok();
        let pending = std::fs::read_to_string(&pending_path).ok();
        let _ = recovery.kill();
        let _ = recovery.wait();
        FileExt::unlock(&route_lock).unwrap();
        panic!(
            "restarted uninstall did not resume at accepted-call drain: status={process_status:?}, lifecycle={lifecycle:?}, snapshot={snapshot:?}, retained={retained:?}, pending={pending:?}"
        );
    }
    let process_status = recovery.try_wait().unwrap();
    let pending_during_drain = pending_path.exists();
    let retained_during_drain = retained_receipt.exists();
    let generation_during_drain = std::fs::read(&snapshot_path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|snapshot| snapshot["generation"].as_u64());
    if process_status.is_some()
        || !pending_during_drain
        || !retained_during_drain
        || generation_during_drain != Some(generation_after_cutover)
    {
        let _ = recovery.kill();
        let _ = recovery.wait();
        FileExt::unlock(&route_lock).unwrap();
        panic!(
            "restarted uninstall crossed the drain boundary early: status={process_status:?}, pending={pending_during_drain}, retained={retained_during_drain}, generation={generation_during_drain:?}"
        );
    }
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);

    let recovered = recovery.wait_with_output().unwrap();
    assert!(recovered.status.success(), "{recovered:?}");
    assert_eq!(json(&recovered)["data"]["changed"], true);
    assert_eq!(
        json(&recovered)["data"]["packageGraph"]["plan"]["plan"]["operationId"],
        operation_id
    );
    assert!(!selected_receipt.exists());
    assert!(!retained_receipt.exists());
    assert!(!pending_path.exists());
    let snapshot_after: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    assert_eq!(snapshot_after["generation"], generation_after_cutover);
    assert!(snapshot_after["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
    let lifecycle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lifecycle_path).unwrap()).unwrap();
    assert_eq!(lifecycle["status"], "completed");
    assert_eq!(
        lifecycle["receipts"].as_array().unwrap().len(),
        lifecycle["intent"]["checkpoints"].as_array().unwrap().len()
    );
}
