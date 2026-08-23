use super::*;

#[cfg(any(unix, windows))]
#[test]
fn killed_registry_resolution_retains_a_path_free_pre_lock_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![package], 83, FUTURE);
    let timestamp_path = "/metadata/timestamp.json";
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(&server, &repository, &home, &[]);
    server.pause_response_after(timestamp_path, 1);
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
    let attempt_path = home.join("state/operations/package-resolutions/install/acme/root.json");
    let reached_resolution = wait_until(Duration::from_secs(15), || {
        attempt_path.is_file()
            && server
                .requests()
                .iter()
                .any(|request| request == timestamp_path)
    });
    if !reached_resolution {
        let process_status = interrupted.try_wait().unwrap();
        let requests = server.requests();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        server.resume_response(timestamp_path);
        panic!(
            "install did not pause during pre-lock TUF resolution: status={process_status:?}, requests={requests:?}"
        );
    }

    let requests_before_diagnostic = server.requests().len();
    let active = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(active.status.success(), "{active:?}");
    let active = json(&active);
    let diagnostic = &active["data"]["diagnostic"];
    assert_eq!(
        diagnostic["schema"],
        "a3s.use.plugin-resolution-attempt-diagnostic.v1"
    );
    assert_eq!(diagnostic["packageId"], "acme/root");
    assert_eq!(diagnostic["scope"]["kind"], "user");
    assert_eq!(diagnostic["scope"]["id"], "user/current");
    assert_eq!(diagnostic["attempt"]["action"], "install");
    assert_eq!(diagnostic["attempt"]["phase"], "pre-lock");
    assert_eq!(diagnostic["attempt"]["access"], "refreshed");
    assert_eq!(diagnostic["attempt"]["status"], "resolving");
    assert_eq!(diagnostic["attempt"]["requestedVersion"], "1.0.0");
    assert_eq!(diagnostic["attempt"]["channel"], "stable");
    assert_eq!(diagnostic["attempt"]["registryCount"], 1);
    assert_eq!(diagnostic["attempt"]["verifiedRegistryCount"], 0);
    assert_eq!(
        diagnostic["attempt"]["registries"][0]["registryName"],
        "fixture"
    );
    assert!(
        diagnostic["attempt"]["registries"][0]["sourceIdentityDigest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:") && digest.len() == 71)
    );
    assert_eq!(diagnostic["attempt"]["registries"][0]["role"], "root");
    assert_eq!(
        diagnostic["attempt"]["registries"][0]["status"],
        "verifying"
    );
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    let encoded = serde_json::to_string(&active).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(!encoded.contains("package-resolutions"));

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    server.resume_response(timestamp_path);

    let retained_bytes = std::fs::read(&attempt_path).unwrap();
    let retained = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(retained.status.success(), "{retained:?}");
    assert_eq!(
        json(&retained)["data"]["diagnostic"]["attempt"]["status"],
        "resolving"
    );

    let mut invalid: serde_json::Value = serde_json::from_slice(&retained_bytes).unwrap();
    invalid["credential"] = serde_json::json!("resolution-secret-sentinel-value");
    std::fs::write(&attempt_path, serde_json::to_vec(&invalid).unwrap()).unwrap();
    let rejected = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!rejected.status.success(), "{rejected:?}");
    let rejected = json(&rejected);
    assert_eq!(
        rejected["error"]["code"],
        "use.plugin.operation_diagnostic_state_invalid"
    );
    let encoded = serde_json::to_string(&rejected).unwrap();
    assert!(!encoded.contains("resolution-secret-sentinel-value"));
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    std::fs::write(&attempt_path, retained_bytes).unwrap();

    server.clear_requests();
    let recovered = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(!attempt_path.exists());
    assert!(home.join("state/extensions/acme/root.json").is_file());
}

#[test]
fn failed_registry_verification_retains_only_a_bounded_error_code() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![package], 89, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(&server, &repository, &home, &[]);
    let mut broken_routes = repository.routes.clone();
    broken_routes.remove("/metadata/timestamp.json");
    server.replace_routes(broken_routes);
    server.clear_requests();

    let failed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(!failed.status.success(), "{failed:?}");

    let requests_before_diagnostic = server.requests().len();
    let diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(diagnostic.status.success(), "{diagnostic:?}");
    let diagnostic = json(&diagnostic);
    let attempt = &diagnostic["data"]["diagnostic"]["attempt"];
    assert_eq!(attempt["phase"], "pre-lock");
    assert_eq!(attempt["status"], "failed");
    assert_eq!(attempt["errorCode"], "use.extension.registry_untrusted");
    assert_eq!(attempt["verifiedRegistryCount"], 0);
    assert_eq!(attempt["registries"][0]["status"], "failed");
    assert_eq!(
        attempt["registries"][0]["errorCode"],
        "use.extension.registry_untrusted"
    );
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    let encoded = serde_json::to_string(&diagnostic).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(attempt.as_object().unwrap().keys().all(|field| matches!(
        field.as_str(),
        "action"
            | "phase"
            | "access"
            | "status"
            | "startedAtMs"
            | "completedAtMs"
            | "requestedVersion"
            | "channel"
            | "registryCount"
            | "verifiedRegistryCount"
            | "packageLockDigest"
            | "packageCount"
            | "errorCode"
            | "registries"
    )));
    assert!(attempt["registries"][0]
        .as_object()
        .unwrap()
        .keys()
        .all(|field| matches!(
            field.as_str(),
            "registryName"
                | "role"
                | "sourceIdentityDigest"
                | "trustRootDigest"
                | "status"
                | "rootVersion"
                | "timestampVersion"
                | "snapshotVersion"
                | "targetsVersion"
                | "packageTargets"
                | "observedAtMs"
                | "errorCode"
        )));
}

#[test]
fn offline_resolution_failure_is_diagnostic_and_constructs_no_network_transport() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![package], 97, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(&server, &repository, &home, &[]);
    server.clear_requests();
    let failed =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(!failed.status.success(), "{failed:?}");
    assert_eq!(
        json(&failed)["error"]["code"],
        "use.extension.catalog_cache_missing"
    );
    assert!(server.requests().is_empty());

    let diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(diagnostic.status.success(), "{diagnostic:?}");
    let diagnostic = json(&diagnostic);
    let attempt = &diagnostic["data"]["diagnostic"]["attempt"];
    assert_eq!(attempt["phase"], "pre-lock");
    assert_eq!(attempt["access"], "cached");
    assert_eq!(attempt["status"], "failed");
    assert_eq!(attempt["errorCode"], "use.extension.catalog_cache_missing");
    assert_eq!(
        attempt["registries"][0]["errorCode"],
        "use.extension.catalog_cache_missing"
    );
    assert!(server.requests().is_empty());
}
