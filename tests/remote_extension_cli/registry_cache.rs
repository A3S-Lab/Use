use super::*;

fn cache_command(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    action: &str,
    extra: &[&str],
) -> Output {
    configure_registry(server, repository, home, &[]);
    Command::new(binary())
        .args(["registry", "cache", action, "--registry-name", "fixture"])
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

#[test]
fn registry_cache_usage_and_confirmed_prune_are_bounded_and_zero_network() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let target_bytes = package.archive.len() as u64;
    let repository = TestRepository::with_targets(vec![package], 71, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");
    let source = registry_source_snapshot(&home)["sources"][0].clone();
    let source_identity = source["sourceIdentity"].as_str().unwrap();
    let cache_directory = home
        .join("state/remote-registries/fixture/sources")
        .join(source_identity)
        .join("verified-targets/sha256");
    std::fs::write(cache_directory.join(".target-999-999.tmp"), b"stale").unwrap();
    std::fs::write(
        cache_directory.join(format!(".target-{}.part", "d".repeat(64))),
        b"partial",
    )
    .unwrap();

    server.clear_requests();
    let usage = cache_command(&server, &repository, &home, "usage", &[]);
    assert!(usage.status.success(), "{usage:?}");
    let usage = json(&usage);
    let cache = &usage["data"]["registryCache"];
    assert_eq!(cache["schemaVersion"], 2);
    assert_eq!(cache["registryName"], "fixture");
    assert_eq!(cache["targetEntries"], 1);
    assert_eq!(cache["targetBytes"], target_bytes);
    assert_eq!(cache["partialEntries"], 1);
    assert_eq!(cache["partialBytes"], 7);
    assert_eq!(cache["staleEntries"], 1);
    assert_eq!(cache["staleBytes"], 5);
    assert!(cache["availableBytes"].as_u64().unwrap() > 0);
    assert!(cache["policy"]["maxBytes"].as_u64().unwrap() >= target_bytes);
    assert!(server.requests().is_empty());

    let replacement_url = format!("{}replacement/", server.base_url());
    let revision = registry_source_snapshot(&home)["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let replaced = Command::new(binary())
        .args([
            "registry",
            "source",
            "replace",
            "fixture",
            "--url",
            &replacement_url,
            "--trust-root",
            &repository.root_sha256,
            "--expected-revision",
            &revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(replaced.status.success(), "{replaced:?}");
    let replacement_usage = cache_command(&server, &repository, &home, "usage", &[]);
    assert!(replacement_usage.status.success(), "{replacement_usage:?}");
    assert_eq!(
        json(&replacement_usage)["data"]["registryCache"]["targetEntries"],
        0
    );
    assert_eq!(std::fs::read_dir(&cache_directory).unwrap().count(), 3);
    assert!(server.requests().is_empty());

    let replacement_revision = registry_source_snapshot(&home)["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    let restored = Command::new(binary())
        .args([
            "registry",
            "source",
            "replace",
            "fixture",
            "--url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
            "--expected-revision",
            &replacement_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(restored.status.success(), "{restored:?}");

    let unconfirmed = cache_command(
        &server,
        &repository,
        &home,
        "prune",
        &["--cache-max-bytes", "1", "--cache-min-free-bytes", "0"],
    );
    assert!(!unconfirmed.status.success(), "{unconfirmed:?}");
    assert_eq!(json(&unconfirmed)["error"]["code"], "use.cli.invalid_usage");
    assert_eq!(std::fs::read_dir(&cache_directory).unwrap().count(), 3);

    let pruned = cache_command(
        &server,
        &repository,
        &home,
        "prune",
        &[
            "--cache-max-bytes",
            "1",
            "--cache-min-free-bytes",
            "0",
            "--yes",
        ],
    );
    assert!(pruned.status.success(), "{pruned:?}");
    let pruned = json(&pruned);
    let cache = &pruned["data"]["registryCache"];
    assert_eq!(cache["removedTargetEntries"], 1);
    assert_eq!(cache["removedTargetBytes"], target_bytes);
    assert_eq!(cache["removedPartialEntries"], 1);
    assert_eq!(cache["removedPartialBytes"], 7);
    assert_eq!(cache["removedStaleEntries"], 1);
    assert_eq!(cache["removedStaleBytes"], 5);
    assert_eq!(cache["after"]["targetEntries"], 0);
    assert_eq!(cache["after"]["targetBytes"], 0);
    assert_eq!(cache["after"]["partialEntries"], 0);
    assert_eq!(cache["after"]["partialBytes"], 0);
    assert_eq!(cache["after"]["staleEntries"], 0);
    assert!(server.requests().is_empty());
}

#[test]
fn registry_cache_policy_rejects_an_oversized_target_before_download() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    assert!(package.archive.len() > 1);
    let repository = TestRepository::with_targets(vec![package], 73, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(
        &server,
        &repository,
        &home,
        &["--cache-max-bytes", "1", "--cache-min-free-bytes", "0"],
    );

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(!installed.status.success(), "{installed:?}");
    assert_eq!(
        json(&installed)["error"]["code"],
        "use.extension.registry_target_cache_policy_exceeded"
    );
    assert_eq!(target_request_count(&server), 0);
    assert!(!scoped_state(&home, "extensions/acme/root.json").exists());
}

#[cfg(any(unix, windows))]
#[test]
fn killed_registry_download_resumes_without_publishing_partial_state() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let archive_bytes = package.archive.len() as u64;
    let repository = TestRepository::with_targets(vec![package], 79, FUTURE);
    let target_path = format!("/targets/{}", repository.target_name);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    configure_registry(&server, &repository, &home, &[]);
    let source = registry_source_snapshot(&home)["sources"][0].clone();
    let source_identity = source["sourceIdentity"].as_str().unwrap();
    let cache_directory = home
        .join("state/remote-registries/fixture/sources")
        .join(source_identity)
        .join("verified-targets/sha256");
    let partial = cache_directory.join(format!(".target-{}.part", repository.target_sha256));
    let verified = cache_directory.join(&repository.target_sha256);
    let pause_after = usize::try_from((archive_bytes / 2).max(1)).unwrap();
    server.pause_response_after(&target_path, pause_after);
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
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_partial = wait_until(Duration::from_secs(15), || {
        std::fs::metadata(&partial)
            .ok()
            .is_some_and(|metadata| metadata.len() > 0 && metadata.len() < archive_bytes)
    });
    if !reached_partial {
        let process_status = interrupted.try_wait().unwrap();
        let partial_length = std::fs::metadata(&partial)
            .ok()
            .map(|metadata| metadata.len());
        let requests = server.requests();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        server.resume_response(&target_path);
        panic!(
            "install did not pause during the Registry target download: status={process_status:?}, partial={partial_length:?}, requests={requests:?}"
        );
    }

    let requests_before_diagnostic = server.requests().len();
    let active_diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(active_diagnostic.status.success(), "{active_diagnostic:?}");
    let active_diagnostic = json(&active_diagnostic);
    let diagnostic = &active_diagnostic["data"]["diagnostic"];
    assert_eq!(
        diagnostic["schema"],
        "a3s.use.plugin-download-attempt-diagnostic.v1"
    );
    assert_eq!(diagnostic["packageId"], "acme/root");
    assert_eq!(diagnostic["scope"]["kind"], "user");
    assert_eq!(diagnostic["scope"]["id"], "user/current");
    assert_eq!(diagnostic["attempt"]["action"], "install");
    assert_eq!(diagnostic["attempt"]["phase"], "pre-plan");
    assert_eq!(diagnostic["attempt"]["packageCount"], 1);
    assert_eq!(diagnostic["attempt"]["downloadBytes"], archive_bytes);
    assert_eq!(diagnostic["attempt"]["downloadTargetCount"], 1);
    assert_eq!(diagnostic["attempt"]["download"], "in-progress");
    assert_eq!(diagnostic["attempt"]["downloads"][0]["status"], "partial");
    let active_retained = diagnostic["attempt"]["downloadRetainedBytes"]
        .as_u64()
        .unwrap();
    assert!(active_retained > 0 && active_retained < archive_bytes);
    assert_eq!(server.requests().len(), requests_before_diagnostic);
    let encoded = serde_json::to_string(&active_diagnostic).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(!encoded.contains("verified-targets"));

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    server.resume_response(&target_path);
    let partial_length = std::fs::metadata(&partial).unwrap().len();
    assert!(partial_length > 0 && partial_length < archive_bytes);
    assert!(!verified.exists());
    assert!(!scoped_state(&home, "extensions/acme/root.json").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "operations/package-graphs/install/acme/root.json").exists());
    assert!(!scoped_data(&home, "extensions/acme/root").exists());

    let attempt_path = scoped_state(&home, "operations/package-downloads/install/acme/root.json");
    let retained_attempt = std::fs::read(&attempt_path).unwrap();
    let mut invalid_attempt: serde_json::Value = serde_json::from_slice(&retained_attempt).unwrap();
    invalid_attempt["credential"] = serde_json::json!("download-secret-sentinel-value");
    std::fs::write(&attempt_path, serde_json::to_vec(&invalid_attempt).unwrap()).unwrap();
    let invalid_diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .for_test_installation()
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
    let encoded = serde_json::to_string(&invalid_diagnostic).unwrap();
    assert!(!encoded.contains("download-secret-sentinel-value"));
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    std::fs::write(&attempt_path, retained_attempt).unwrap();

    let retained_diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        retained_diagnostic.status.success(),
        "{retained_diagnostic:?}"
    );
    let retained_diagnostic = json(&retained_diagnostic);
    assert_eq!(
        retained_diagnostic["data"]["diagnostic"]["attempt"]["downloadRetainedBytes"],
        partial_length
    );
    assert_eq!(
        retained_diagnostic["data"]["diagnostic"]["attempt"]["download"],
        "in-progress"
    );

    server.clear_requests();
    let recovered = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert_eq!(
        server.ranges_for(&target_path),
        vec![format!("bytes={partial_length}-")]
    );
    assert!(!partial.exists());
    assert!(verified.is_file());
    assert!(scoped_state(&home, "extensions/acme/root.json").is_file());
    assert!(scoped_state(&home, "installation-snapshot.json").is_file());
    let completed_diagnostic = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        !completed_diagnostic.status.success(),
        "{completed_diagnostic:?}"
    );
    assert_eq!(
        json(&completed_diagnostic)["error"]["code"],
        "use.plugin.operation_diagnostic_not_found"
    );
}
