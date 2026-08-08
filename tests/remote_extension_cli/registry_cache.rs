use super::*;

fn cache_command(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    action: &str,
    extra: &[&str],
) -> Output {
    Command::new(binary())
        .args([
            "registry",
            "cache",
            action,
            "--registry-name",
            "fixture",
            "--registry-url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
        ])
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
    let cache_directory = home.join("state/remote-registries/fixture/verified-targets/sha256");
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

    let mismatched_source = Command::new(binary())
        .args([
            "registry",
            "cache",
            "usage",
            "--registry-name",
            "fixture",
            "--registry-url",
            &format!("{}replacement/", server.base_url()),
            "--trust-root",
            &repository.root_sha256,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!mismatched_source.status.success(), "{mismatched_source:?}");
    assert_eq!(
        json(&mismatched_source)["error"]["code"],
        "use.extension.catalog_cache_invalid"
    );
    assert!(server.requests().is_empty());

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

    let installed = cognitive_registry_install(
        &server,
        &repository,
        &home,
        "acme/root",
        &["--cache-max-bytes", "1", "--cache-min-free-bytes", "0"],
    );
    assert!(!installed.status.success(), "{installed:?}");
    assert_eq!(
        json(&installed)["error"]["code"],
        "use.extension.registry_target_cache_policy_exceeded"
    );
    assert_eq!(target_request_count(&server), 0);
    assert!(!home.join("state/extensions/acme/root.json").exists());
}
