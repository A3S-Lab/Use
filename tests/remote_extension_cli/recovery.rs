use super::*;

#[cfg(unix)]
#[test]
fn schema_v3_uninstall_replays_after_the_root_and_graph_record_are_removed() {
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
    let base_generation =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&base_receipt).unwrap())
            .unwrap()["lifecycleGeneration"]
            .as_u64()
            .unwrap();
    let route_lock = exclusive_lock(
        &home
            .join("state/route-locks/acme/base")
            .join(format!("{base_generation:020}.lock")),
    );
    let mut interrupted = Command::new(binary())
        .args(["uninstall", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_dependency_drain = wait_until(Duration::from_secs(10), || {
        !root_receipt.exists() && base_receipt.exists() && pending_path.exists()
    });
    if !reached_dependency_drain {
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        FileExt::unlock(&route_lock).unwrap();
        panic!("uninstall did not reach the dependency drain checkpoint");
    }
    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    FileExt::unlock(&route_lock).unwrap();
    drop(route_lock);

    assert!(!root_receipt.exists());
    assert!(base_receipt.exists());
    assert!(pending_path.exists());
    std::fs::remove_file(&graph_path).unwrap();

    let recovered = cognitive_uninstall(&home, "acme/root");
    assert!(recovered.status.success(), "{recovered:?}");
    assert_eq!(
        json(&recovered)["data"]["packageGraph"]["removedPackages"],
        serde_json::json!(["acme/root", "acme/base"])
    );
    assert!(!root_receipt.exists());
    assert!(!base_receipt.exists());
    assert!(!graph_path.exists());
    assert!(!pending_path.exists());
}

#[cfg(unix)]
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
            "--registry-url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
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
