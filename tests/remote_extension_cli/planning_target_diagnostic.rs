use super::graph_grants::cognitive_tool_targets_version_with_payload;
use super::*;

#[test]
fn killed_planning_target_download_is_diagnostic_and_hands_off_without_a_gap() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let targets = cognitive_tool_targets_version_with_payload(
        temp.path(),
        "acme/worker",
        "worker",
        "1.0.0",
        &target,
        0,
    );
    let catalog = targets
        .iter()
        .find_map(|target| target.custom.clone())
        .map(|value| PluginCatalogRecord::from_json(&serde_json::to_vec(&value).unwrap()).unwrap())
        .unwrap();
    let planning = catalog.planning.clone().unwrap();
    let planning_bytes = planning.length;
    let planning_digest = planning.sha256.strip_prefix("sha256:").unwrap();
    let planning_path = format!("/targets/{}", planning.target_name);
    let repository = TestRepository::with_targets(targets, 139, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(&server, &repository, &home, &[]);
    let source = registry_source_snapshot(&home)["sources"][0].clone();
    let source_identity = source["sourceIdentity"].as_str().unwrap();
    let cache_directory = home
        .join("state/remote-registries/fixture/sources")
        .join(source_identity)
        .join("verified-targets/sha256");
    let partial = cache_directory.join(format!(".target-{planning_digest}.part"));
    let verified = cache_directory.join(planning_digest);
    let pause_after = usize::try_from((planning_bytes / 2).max(1)).unwrap();
    server.pause_response_after(&planning_path, pause_after);
    server.clear_requests();

    let mut interrupted = Command::new(binary())
        .args([
            "install",
            "acme/worker",
            "--registry-name",
            "fixture",
            "--version",
            "1.0.0",
            "--json",
        ])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_partial = wait_until(Duration::from_secs(15), || {
        std::fs::metadata(&partial)
            .ok()
            .is_some_and(|metadata| metadata.len() > 0 && metadata.len() < planning_bytes)
    });
    if !reached_partial {
        let process_status = interrupted.try_wait().unwrap();
        let partial_length = std::fs::metadata(&partial)
            .ok()
            .map(|metadata| metadata.len());
        let requests = server.requests();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        server.resume_response(&planning_path);
        panic!(
            "install did not pause during the planning-target download: status={process_status:?}, partial={partial_length:?}, requests={requests:?}"
        );
    }

    let requests_before_diagnostic = server.requests().len();
    let active = Command::new(binary())
        .args(["extension", "diagnose", "acme/worker", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(active.status.success(), "{active:?}");
    let active = json(&active);
    let diagnostic = &active["data"]["diagnostic"];
    assert_eq!(
        diagnostic["schema"],
        "a3s.use.plugin-download-attempt-diagnostic.v1"
    );
    assert_eq!(diagnostic["attempt"]["download"], "missing");
    assert_eq!(diagnostic["attempt"]["planningBytes"], planning_bytes);
    assert_eq!(diagnostic["attempt"]["planningTargetCount"], 1);
    assert_eq!(diagnostic["attempt"]["planning"], "in-progress");
    assert_eq!(
        diagnostic["attempt"]["planningTargets"][0]["packageId"],
        "acme/worker"
    );
    assert_eq!(
        diagnostic["attempt"]["planningTargets"][0]["targetDigest"],
        planning.sha256
    );
    assert_eq!(
        diagnostic["attempt"]["planningTargets"][0]["status"],
        "partial"
    );
    let active_retained = diagnostic["attempt"]["planningRetainedBytes"]
        .as_u64()
        .unwrap();
    assert!(active_retained > 0 && active_retained < planning_bytes);
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    let encoded = serde_json::to_string(&active).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(!encoded.contains(&planning.target_name));

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    server.resume_response(&planning_path);
    let partial_length = std::fs::metadata(&partial).unwrap().len();
    assert!(partial_length > 0 && partial_length < planning_bytes);
    assert!(!verified.exists());
    assert!(!scoped_state(&home, "operations/package-graphs/install/acme/worker.json").exists());

    let retained = Command::new(binary())
        .args(["extension", "diagnose", "acme/worker", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(retained.status.success(), "{retained:?}");
    assert_eq!(
        json(&retained)["data"]["diagnostic"]["attempt"]["planningRetainedBytes"],
        partial_length
    );

    server.clear_requests();
    let resumed = cognitive_registry_install(&server, &repository, &home, "acme/worker", &[]);
    if !resumed.status.success() {
        assert_eq!(
            json(&resumed)["error"]["code"],
            "use.plugin.package_confirmation_required"
        );
    }
    assert_eq!(
        server.ranges_for(&planning_path),
        vec![format!("bytes={partial_length}-")]
    );
    assert!(!partial.exists());
    assert!(verified.is_file());
    assert!(!scoped_state(
        &home,
        "operations/package-downloads/install/acme/worker.json"
    )
    .exists());
    assert!(scoped_state(&home, "operations/package-graphs/install/acme/worker.json").is_file());

    let handed_off = Command::new(binary())
        .args(["extension", "diagnose", "acme/worker", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(handed_off.status.success(), "{handed_off:?}");
    let handed_off = json(&handed_off);
    let operation = &handed_off["data"]["diagnostic"]["operation"];
    assert_eq!(operation["phase"], "planned");
    assert_eq!(operation["planningBytes"], planning_bytes);
    assert_eq!(operation["planningRetainedBytes"], planning_bytes);
    assert_eq!(operation["planningTargetCount"], 1);
    assert_eq!(operation["planning"], "complete");
    assert_eq!(operation["planningTargets"][0]["status"], "complete");
}
