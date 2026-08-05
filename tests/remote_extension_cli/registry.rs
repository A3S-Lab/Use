use super::*;

#[tokio::test]
async fn signed_registry_install_uses_reviewed_target_and_reports_tuf_provenance() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let trusted = TrustedRegistry::new(
        "fixture",
        server.base_url(),
        &repository.root_sha256,
        None,
        temp.path().join("review-state"),
    )
    .unwrap();
    let reviewed = prepare_remote_package(&trusted, "a3s/science", None, "stable", None)
        .await
        .unwrap();
    let plan_digest = reviewed.resolved().plan_digest().unwrap();
    drop(reviewed);
    assert_no_target_request(&server);

    let home = temp.path().join("home");
    let installed = registry_install(&server, &repository, &home, Some(&plan_digest), &[]);
    assert!(installed.status.success(), "{installed:?}");
    let installed_json = json(&installed);
    assert_eq!(installed_json["data"]["changed"], true);
    assert_eq!(installed_json["data"]["component"]["trust"], "registry-tuf");
    assert_eq!(
        installed_json["data"]["component"]["registry"]["registryName"],
        "fixture"
    );
    assert_eq!(
        installed_json["data"]["component"]["registry"]["sha256"],
        repository.target_sha256
    );
    assert_eq!(
        server
            .requests()
            .iter()
            .filter(|request| request.starts_with("/targets/"))
            .count(),
        1
    );

    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join("state/extensions/a3s/science.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["trust"], "registry-tuf");
    let provenance: ResolvedRemotePackage =
        serde_json::from_value(receipt["registry"].clone()).unwrap();
    assert_eq!(provenance.plan_digest().unwrap(), plan_digest);

    let inspected = Command::new(binary())
        .args(["extension", "inspect", "a3s/science", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(inspected.status.success(), "{inspected:?}");
    let inspected = json(&inspected);
    assert_eq!(inspected["data"]["extension"]["trust"], "registry-tuf");
    assert_eq!(
        inspected["data"]["extension"]["registry"]["targetName"],
        repository.target_name
    );

    let second = registry_install(&server, &repository, &home, Some(&plan_digest), &[]);
    assert!(second.status.success(), "{second:?}");
    assert_eq!(json(&second)["data"]["changed"], false);
}

#[test]
fn registry_plan_mismatch_fails_before_target_download() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let output = registry_install(
        &server,
        &repository,
        &temp.path().join("home"),
        Some(&"0".repeat(64)),
        &[],
    );

    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.extension.registry_plan_mismatch"
    );
    assert_no_target_request(&server);
}

#[test]
fn registry_install_rejects_unsigned_and_local_source_combinations() {
    let repository = TestRepository::new(extension_archive(PACKAGE_VERSION), 1, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");

    let unsigned = registry_install(&server, &repository, &home, None, &["--allow-unsigned"]);
    assert!(!unsigned.status.success(), "{unsigned:?}");
    assert_eq!(json(&unsigned)["error"]["code"], "use.cli.invalid_usage");

    let local = Command::new(binary())
        .args([
            "component",
            "install",
            "a3s/science",
            "--from",
            temp.path().to_str().unwrap(),
            "--allow-unsigned",
            "--registry-name",
            "fixture",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!local.status.success(), "{local:?}");
    assert_eq!(json(&local)["error"]["code"], "use.cli.invalid_usage");
    assert!(server.requests().is_empty());
}

#[test]
fn schema_v3_okf_install_fails_before_any_package_becomes_visible_without_knowledge() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let archive = package_directory_archive(&package_root);
    let mut catalog: serde_json::Value = serde_json::from_slice(OKF_CATALOG_V3).unwrap();
    catalog["target"] = serde_json::json!(&target);
    catalog["archive"]["targetName"] = serde_json::json!(format!(
        "extensions/acme/knowledge/1.0.0/stable/{target}/acme-knowledge-1.0.0-{target}.tar.gz"
    ));
    let target_name = catalog["archive"]["targetName"]
        .as_str()
        .unwrap()
        .to_string();
    let repository =
        TestRepository::with_target_metadata(archive, target_name, catalog, 17, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let output = cognitive_registry_install(&server, &repository, &home, "acme/knowledge", &[]);
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.plugin.okf_provider_required"
    );
    assert!(!home.join("state/extensions/acme/knowledge.json").exists());
    let snapshot = home.join("state/registry.json");
    if snapshot.exists() {
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(snapshot).unwrap()).unwrap();
        assert_eq!(snapshot["routes"], serde_json::json!([]));
    }
}

#[test]
fn schema_v3_lock_mismatch_fails_before_any_archive_download() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let root = cognitive_skill_target(temp.path(), "acme/root", "root", Vec::new(), &target);
    let repository = TestRepository::with_targets(vec![root], 19, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let output = cognitive_registry_install(
        &server,
        &repository,
        &home,
        "acme/root",
        &["--package-lock-digest", &"0".repeat(64)],
    );
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.plugin.package_lock_mismatch"
    );
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}
