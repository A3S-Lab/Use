use super::*;

const GRAPH_DEPENDENCY_COUNT: usize = 8;

#[test]
fn killed_graph_publication_replays_exact_cutover_offline_without_generation_inflation() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let expected_package_ids = (0..GRAPH_DEPENDENCY_COUNT)
        .map(|index| format!("acme/leaf-{index:02}"))
        .chain(std::iter::once("acme/root".to_owned()))
        .collect::<std::collections::BTreeSet<_>>();
    let dependencies = (0..GRAPH_DEPENDENCY_COUNT)
        .map(|index| {
            PluginPackageDependency::new(format!("acme/leaf-{index:02}"), "^1.0.0").unwrap()
        })
        .collect::<Vec<_>>();
    let mut targets = dependencies
        .iter()
        .enumerate()
        .map(|(index, dependency)| {
            cognitive_skill_target(
                &temp.path().join(format!("leaf-{index:02}")),
                &dependency.package_id,
                &format!("leaf-{index:02}"),
                Vec::new(),
                &target,
            )
        })
        .collect::<Vec<_>>();
    targets.push(cognitive_skill_target(
        &temp.path().join("root"),
        "acme/root",
        "root",
        dependencies,
        &target,
    ));
    let repository = TestRepository::with_targets(targets, 103, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let pending_path = home.join("state/operations/package-graphs/install/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");
    let snapshot_path = home.join("state/registry.json");
    let held_journal = lifecycle_journal_path(&home, "acme/leaf-00");

    configure_registry(&server, &repository, &home, &[]);
    server.clear_requests();
    let mut interrupted = Command::new(binary())
        .args([
            "install",
            "acme/root",
            "--registry-name",
            "fixture",
            "--version",
            "1.0.0",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();

    let reached_prepare = wait_for_lifecycle_prepare(&held_journal);
    if !reached_prepare {
        let process_status = interrupted.try_wait().unwrap();
        let pending_bytes = file_length(&pending_path);
        let lifecycle = lifecycle_summary(&held_journal);
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        panic!(
            "install did not expose the complete dependency prepare checkpoint: status={process_status:?}, pending_bytes={pending_bytes:?}, lifecycle={lifecycle:?}"
        );
    }

    let lifecycle_lock = exclusive_lock(&held_journal.with_file_name(".operation.lock"));
    let still_applying = lifecycle_is_prepared(&held_journal);
    if !still_applying {
        let process_status = interrupted.try_wait().unwrap();
        let lifecycle = lifecycle_summary(&held_journal);
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "dependency lifecycle completed before its operation lock was acquired: status={process_status:?}, lifecycle={lifecycle:?}"
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
                .is_some_and(|routes| routes.len() == GRAPH_DEPENDENCY_COUNT + 1)
            && snapshot["pendingCutovers"]
                .as_array()
                .is_some_and(|cutovers| cutovers.len() == 1)
    });
    if !reached_cutover {
        let process_status = interrupted.try_wait().unwrap();
        let snapshot = read_json(&snapshot_path);
        let pending_bytes = file_length(&pending_path);
        let lifecycle = lifecycle_summary(&held_journal);
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        FileExt::unlock(&lifecycle_lock).unwrap();
        panic!(
            "install did not reach the graph cutover-before-journal checkpoint: status={process_status:?}, snapshot={snapshot:?}, pending_bytes={pending_bytes:?}, lifecycle={lifecycle:?}"
        );
    }

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    FileExt::unlock(&lifecycle_lock).unwrap();
    drop(lifecycle_lock);

    let snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(snapshot["generation"], 1);
    assert_eq!(
        snapshot["routes"].as_array().unwrap().len(),
        GRAPH_DEPENDENCY_COUNT + 1
    );
    assert_eq!(route_package_ids(&snapshot), expected_package_ids);
    assert_eq!(snapshot["pendingCutovers"].as_array().unwrap().len(), 1);
    assert_eq!(lifecycle_status(&held_journal).as_deref(), Some("applying"));
    assert!(pending_path.is_file());
    assert!(!graph_path.exists());
    for package_id in &expected_package_ids {
        let receipt = read_json(
            &home
                .join("state/extensions")
                .join(format!("{package_id}.json")),
        )
        .unwrap();
        assert_eq!(receipt["packageId"], package_id.as_str());
        assert_eq!(receipt["enabled"], true);
    }

    let requests_before_diagnostic = server.requests().len();
    let diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(diagnostic.status.success(), "{diagnostic:?}");
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    let diagnostic = json(&diagnostic);
    let diagnostic = &diagnostic["data"]["diagnostic"];
    assert_eq!(diagnostic["operation"]["phase"], "admitted");
    assert_eq!(diagnostic["operation"]["action"], "install");
    assert_eq!(
        diagnostic["operation"]["lifecycleUnitCount"],
        GRAPH_DEPENDENCY_COUNT + 1
    );
    assert_eq!(
        diagnostic["operation"]["observedLifecycleUnitCount"],
        GRAPH_DEPENDENCY_COUNT + 1
    );
    assert_eq!(
        diagnostic["operation"]["lifecycle"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|unit| unit["publication"] == "pending")
            .count(),
        GRAPH_DEPENDENCY_COUNT + 1
    );
    assert_eq!(
        diagnostic["registry"]["operationCutover"]["status"],
        "recorded"
    );
    assert_eq!(diagnostic["registry"]["pendingCutoverCount"], 1);
    assert_eq!(
        diagnostic["registry"]["operationCutover"]["recordedGenerationAfter"],
        1
    );
    assert_eq!(diagnostic["operation"]["recovery"], "resume-exact-plan");

    server.clear_requests();
    let recovered =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(server.requests().is_empty());
    assert!(!pending_path.exists());
    assert!(graph_path.is_file());
    let installed_graph = read_json(&graph_path).unwrap();
    assert_eq!(graph_package_ids(&installed_graph), expected_package_ids);
    for package_id in &expected_package_ids {
        assert_eq!(
            lifecycle_status(&lifecycle_journal_path(&home, package_id)).as_deref(),
            Some("completed")
        );
    }

    let recovered_snapshot = read_json(&snapshot_path).unwrap();
    assert_eq!(recovered_snapshot["generation"], 1);
    assert_eq!(
        recovered_snapshot["routes"].as_array().unwrap().len(),
        GRAPH_DEPENDENCY_COUNT + 1
    );
    assert_eq!(route_package_ids(&recovered_snapshot), expected_package_ids);
    assert!(recovered_snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
}

fn wait_for_lifecycle_prepare(path: &std::path::Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if lifecycle_is_prepared(path) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    lifecycle_is_prepared(path)
}

fn lifecycle_is_prepared(path: &std::path::Path) -> bool {
    read_json(path).is_some_and(|journal| {
        let receipt_count = journal["receipts"].as_array().map_or(0, Vec::len);
        let checkpoint_count = journal["intent"]["checkpoints"]
            .as_array()
            .map_or(0, Vec::len);
        journal["status"] == "applying"
            && checkpoint_count > 0
            && receipt_count + 1 == checkpoint_count
    })
}

fn lifecycle_status(path: &std::path::Path) -> Option<String> {
    read_json(path)?["status"].as_str().map(str::to_owned)
}

fn read_json(path: &std::path::Path) -> Option<serde_json::Value> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
}

fn lifecycle_summary(path: &std::path::Path) -> Option<(String, usize, usize)> {
    let journal = read_json(path)?;
    Some((
        journal["status"].as_str()?.to_owned(),
        journal["receipts"].as_array().map_or(0, Vec::len),
        journal["intent"]["checkpoints"]
            .as_array()
            .map_or(0, Vec::len),
    ))
}

fn file_length(path: &std::path::Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|metadata| metadata.len())
}

fn route_package_ids(snapshot: &serde_json::Value) -> std::collections::BTreeSet<String> {
    snapshot["routes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|route| route["packageId"].as_str().unwrap().to_owned())
        .collect()
}

fn graph_package_ids(graph: &serde_json::Value) -> std::collections::BTreeSet<String> {
    graph["packageLock"]["packages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|package| {
            package["catalog"]["record"]["packageId"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect()
}
