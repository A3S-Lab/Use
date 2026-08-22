use super::*;

#[test]
fn completed_operations_have_bounded_zero_network_history_after_package_removal() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![package], 107, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("private-history-home-marker");

    let install = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(install.status.success(), "{install:?}");
    let requests_before_history = server.requests().len();
    let first_history = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--history", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(first_history.status.success(), "{first_history:?}");
    assert_eq!(server.requests().len(), requests_before_history);
    let first_history = json(&first_history);
    let first_history = &first_history["data"]["diagnostic"];
    assert_eq!(
        first_history["schema"],
        "a3s.use.plugin-operation-history-diagnostic.v1"
    );
    assert_eq!(first_history["retentionLimit"], 16);
    assert_eq!(first_history["retentionByteLimit"], 8 * 1024 * 1024);
    assert_eq!(first_history["retainedOperationCount"], 1);
    assert_eq!(
        first_history["operations"][0]["diagnostic"]["operation"]["action"],
        "install"
    );
    assert_eq!(first_history["operations"][0]["outcome"], "completed");
    assert_eq!(
        first_history["operations"][0]["diagnostic"]["operation"]["phase"],
        "admitted"
    );
    assert_eq!(
        first_history["operations"][0]["diagnostic"]["operation"]["download"],
        "complete"
    );

    let uninstall = cognitive_uninstall(&home, "acme/root");
    assert!(uninstall.status.success(), "{uninstall:?}");
    let requests_before_history = server.requests().len();
    let removed_history = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--history", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(removed_history.status.success(), "{removed_history:?}");
    assert_eq!(server.requests().len(), requests_before_history);
    let removed_history = json(&removed_history);
    let removed_history = &removed_history["data"]["diagnostic"];
    assert_eq!(removed_history["retainedOperationCount"], 2);
    assert_eq!(
        removed_history["operations"][0]["diagnostic"]["operation"]["action"],
        "uninstall"
    );
    assert_eq!(
        removed_history["operations"][1]["diagnostic"]["operation"]["action"],
        "install"
    );

    let reinstall = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(reinstall.status.success(), "{reinstall:?}");
    let requests_before_history = server.requests().len();
    let reinstalled_history = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--history", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        reinstalled_history.status.success(),
        "{reinstalled_history:?}"
    );
    assert_eq!(server.requests().len(), requests_before_history);
    let reinstalled_history = json(&reinstalled_history);
    let reinstalled_history = &reinstalled_history["data"]["diagnostic"];
    assert_eq!(reinstalled_history["retainedOperationCount"], 3);
    assert_eq!(
        reinstalled_history["operations"][0]["diagnostic"]["operation"]["action"],
        "install"
    );
    assert_eq!(
        reinstalled_history["operations"][2]["diagnostic"]["operation"]["action"],
        "install"
    );
    assert_eq!(
        reinstalled_history["operations"][0]["diagnostic"]["operation"]["operationId"],
        reinstalled_history["operations"][2]["diagnostic"]["operation"]["operationId"]
    );
    assert_ne!(
        reinstalled_history["operations"][0]["diagnostic"]["operation"]["planDigest"],
        reinstalled_history["operations"][2]["diagnostic"]["operation"]["planDigest"]
    );
    let encoded = serde_json::to_string(reinstalled_history).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(!encoded.contains("package-diagnostic-history"));
    assert!(!encoded.contains("idempotency"));

    let scope_digest = format!("{:x}", Sha256::digest(b"user\nuser/current"));
    let history_path = home
        .join("state/operations/package-diagnostic-history/scopes")
        .join(scope_digest)
        .join("acme/root.json");
    let mut damaged: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&history_path).unwrap()).unwrap();
    damaged.as_object_mut().unwrap().insert(
        "credential".to_owned(),
        serde_json::json!("history-secret-sentinel"),
    );
    std::fs::write(&history_path, serde_json::to_vec(&damaged).unwrap()).unwrap();
    let invalid = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--history", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!invalid.status.success(), "{invalid:?}");
    let invalid = json(&invalid);
    assert_eq!(
        invalid["error"]["code"],
        "use.plugin.operation_diagnostic_state_invalid"
    );
    let encoded = serde_json::to_string(&invalid).unwrap();
    assert!(!encoded.contains("history-secret-sentinel"));
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
}

#[tokio::test]
async fn reviewed_graph_plan_has_a_bounded_path_free_operation_diagnostic() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![package], 109, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("private-home-marker");
    configure_registry(&server, &repository, &home, &[]);
    let paths = ExtensionPaths::new(home.join("data"), home.join("state"));
    let resolved = a3s_use_extension::RegistrySourceStore::new(paths.clone())
        .resolve(Some("fixture"))
        .await
        .unwrap();
    let trusted = resolved.root().clone();
    let lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/root",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let lock_digest = lock.descriptor_digest().unwrap();
    let registry = ExtensionRegistry::new(paths);
    let manager = CognitivePackageManager::new(registry).unwrap();
    let plan = manager
        .prepare_install_remote(
            &trusted,
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &lock_digest,
        )
        .await
        .unwrap();
    let requests_before_diagnostic = server.requests().len();

    let output = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(server.requests().len(), requests_before_diagnostic);

    let value = json(&output);
    let diagnostic = &value["data"]["diagnostic"];
    assert_eq!(
        diagnostic["schema"],
        "a3s.use.plugin-operation-diagnostic.v1"
    );
    assert_eq!(diagnostic["packageId"], "acme/root");
    assert_eq!(diagnostic["scope"]["kind"], "user");
    assert_eq!(diagnostic["scope"]["id"], "user/current");
    assert_eq!(
        diagnostic["operation"]["operationId"],
        plan.plan.operation_id
    );
    assert_eq!(diagnostic["operation"]["planDigest"], plan.plan_digest);
    assert_eq!(diagnostic["operation"]["action"], "install");
    assert_eq!(diagnostic["operation"]["phase"], "planned");
    assert_eq!(diagnostic["operation"]["packageCount"], 1);
    assert_eq!(diagnostic["operation"]["changedPackageCount"], 1);
    assert_eq!(
        diagnostic["operation"]["downloadBytes"],
        lock.packages[0].catalog.record.archive.length
    );
    assert_eq!(
        diagnostic["operation"]["downloadRetainedBytes"],
        lock.packages[0].catalog.record.archive.length
    );
    assert_eq!(diagnostic["operation"]["downloadTargetCount"], 1);
    assert_eq!(diagnostic["operation"]["download"], "complete");
    assert_eq!(
        diagnostic["operation"]["downloads"][0]["packageId"],
        "acme/root"
    );
    assert_eq!(
        diagnostic["operation"]["downloads"][0]["archiveDigest"],
        lock.packages[0].catalog.record.archive.sha256
    );
    assert_eq!(
        diagnostic["operation"]["downloads"][0]["status"],
        "complete"
    );
    assert_eq!(diagnostic["operation"]["providerCount"], 0);
    assert_eq!(diagnostic["operation"]["grant"]["status"], "not-required");
    assert_eq!(diagnostic["operation"]["lifecycle"], serde_json::json!([]));
    assert_eq!(diagnostic["registry"]["generation"], 0);
    assert!(diagnostic["registry"]["snapshotDigest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(diagnostic["operation"]["sources"][0]["kind"], "registry");
    assert_eq!(
        diagnostic["operation"]["sources"][0]["registryName"],
        "fixture"
    );
    assert_eq!(
        diagnostic["operation"]["sources"][0]["catalogRecordDigest"],
        lock.packages[0].catalog.provenance.catalog_record_digest
    );
    assert_eq!(
        diagnostic["operation"]["sources"][0]["rootVersion"],
        lock.packages[0].catalog.provenance.root_version
    );

    let encoded = serde_json::to_string(&value).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(!encoded.contains("skills/main/SKILL.md"));
    assert!(!encoded.contains("idempotency"));

    let source = registry_source_snapshot(&home)["sources"][0].clone();
    let source_identity = source["sourceIdentity"].as_str().unwrap();
    let cache = home
        .join("state/remote-registries/fixture/sources")
        .join(source_identity)
        .join("verified-targets/sha256");
    let archive_digest = lock.packages[0]
        .catalog
        .record
        .archive
        .sha256
        .strip_prefix("sha256:")
        .unwrap();
    let complete_path = cache.join(archive_digest);
    let complete_bytes = std::fs::read(&complete_path).unwrap();
    std::fs::remove_file(&complete_path).unwrap();

    let missing = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(missing.status.success(), "{missing:?}");
    let missing = json(&missing);
    assert_eq!(
        missing["data"]["diagnostic"]["operation"]["download"],
        "missing"
    );
    assert_eq!(
        missing["data"]["diagnostic"]["operation"]["downloadRetainedBytes"],
        0
    );
    assert_eq!(
        missing["data"]["diagnostic"]["operation"]["downloads"][0]["status"],
        "missing"
    );

    let partial_bytes = complete_bytes.len().div_ceil(2).max(1);
    std::fs::write(
        cache.join(format!(".target-{archive_digest}.part")),
        &complete_bytes[..partial_bytes],
    )
    .unwrap();
    let partial = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(partial.status.success(), "{partial:?}");
    let partial = json(&partial);
    assert_eq!(
        partial["data"]["diagnostic"]["operation"]["download"],
        "in-progress"
    );
    assert_eq!(
        partial["data"]["diagnostic"]["operation"]["downloadRetainedBytes"],
        partial_bytes
    );
    assert_eq!(
        partial["data"]["diagnostic"]["operation"]["downloads"][0]["status"],
        "partial"
    );
    assert_eq!(server.requests().len(), requests_before_diagnostic);

    std::fs::write(&complete_path, &complete_bytes).unwrap();
    let invalid = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!invalid.status.success(), "{invalid:?}");
    let invalid = json(&invalid);
    assert_eq!(
        invalid["error"]["code"],
        "use.plugin.operation_diagnostic_state_invalid"
    );
    assert!(invalid["error"]["suggestion"]
        .as_str()
        .unwrap()
        .contains("reinstall"));
    let encoded = serde_json::to_string(&invalid).unwrap();
    assert!(!encoded.contains(home.to_str().unwrap()));
    assert!(!encoded.contains(server.base_url()));
    assert!(!encoded.contains("verified-targets"));
    assert_eq!(server.requests().len(), requests_before_diagnostic);
}

#[tokio::test]
async fn operation_diagnostic_sanitizes_invalid_pending_state() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![package], 113, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("private-home-marker");
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        home.join("state/remote-registries/fixture"),
    )
    .unwrap();
    let lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "acme/root",
        Some("1.0.0"),
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(target, env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let lock_digest = lock.descriptor_digest().unwrap();
    let registry =
        ExtensionRegistry::new(ExtensionPaths::new(home.join("data"), home.join("state")));
    CognitivePackageManager::new(registry)
        .unwrap()
        .prepare_install_remote(
            &trusted,
            &[],
            "acme/root",
            Some("1.0.0"),
            PluginReleaseChannel::Stable,
            &lock_digest,
        )
        .await
        .unwrap();

    let pending_path = home.join("state/operations/package-graphs/install/acme/root.json");
    let mut pending: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&pending_path).unwrap()).unwrap();
    pending["credential"] = serde_json::json!("secret-sentinel-value");
    std::fs::write(&pending_path, serde_json::to_vec(&pending).unwrap()).unwrap();

    let output = Command::new(binary())
        .args(["extension", "diagnose", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!output.status.success(), "{output:?}");
    let value = json(&output);
    assert_eq!(
        value["error"]["code"],
        "use.plugin.operation_diagnostic_state_invalid"
    );
    assert!(value["error"]["suggestion"]
        .as_str()
        .unwrap()
        .contains("reinstall"));
    let encoded = serde_json::to_string(&value).unwrap();
    assert!(!encoded.contains("secret-sentinel-value"));
    assert!(!encoded.contains(home.to_str().unwrap()));
}
