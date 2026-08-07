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
    let target_plan_digest = reviewed.resolved().plan_digest().unwrap();
    drop(reviewed);
    let lock = resolve_remote_package_lock(
        &trusted,
        &[],
        "a3s/science",
        None,
        PluginReleaseChannel::Stable,
        PluginPackageLockHost::new(host_target(), env!("CARGO_PKG_VERSION")).unwrap(),
    )
    .await
    .unwrap();
    let package_lock_digest = lock.descriptor_digest().unwrap();
    assert_no_target_request(&server);

    let home = temp.path().join("home");
    let installed = registry_install(&server, &repository, &home, Some(&package_lock_digest), &[]);
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
    assert_eq!(provenance.plan_digest().unwrap(), target_plan_digest);

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

    let second = registry_install(&server, &repository, &home, Some(&package_lock_digest), &[]);
    assert!(second.status.success(), "{second:?}");
    assert_eq!(json(&second)["data"]["changed"], false);
}

#[test]
fn package_lock_mismatch_fails_before_target_download() {
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
        "use.plugin.package_lock_mismatch"
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
fn signed_okf_package_installs_queries_and_uninstalls_through_production_knowledge() {
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
    assert!(output.status.success(), "{output:?}");
    assert_eq!(json(&output)["data"]["changed"], true);
    assert!(home.join("state/extensions/acme/knowledge.json").exists());

    let snapshot = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot = json(&snapshot);
    let capability = snapshot["data"]["registry"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["route"] == "knowledge")
        .unwrap_or_else(|| panic!("missing Knowledge capability: {snapshot:#}"));
    assert_eq!(capability["enabled"], true);
    assert_eq!(capability["knowledge"][0]["generation"], 1);
    assert_eq!(capability["knowledge"][0]["scope"]["kind"], "user");

    let searched = Command::new(binary())
        .args([
            "knowledge",
            "search",
            "package activation",
            "--limit",
            "5",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(searched.status.success(), "{searched:?}");
    let searched = json(&searched);
    assert_eq!(
        searched["data"]["knowledge"]["hits"][0]["citation"]["path"],
        "concepts/package-lifecycle.md"
    );
    assert_eq!(
        searched["data"]["knowledge"]["hits"][0]["citation"]["surface"]["packageId"],
        "acme/knowledge"
    );

    let usage = knowledge_usage(&home);
    assert!(usage.status.success(), "{usage:?}");
    let storage = &json(&usage)["data"]["knowledge"]["storage"];
    assert_eq!(storage["retainedProjections"], 1);
    assert_eq!(storage["removedTombstones"], 0);
    assert!(storage["retainedExpandedBytes"].as_u64().unwrap() > 0);

    let audited = Command::new(binary())
        .args(["knowledge", "audit", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(audited.status.success(), "{audited:?}");
    let integrity = &json(&audited)["data"]["knowledge"]["integrity"];
    assert_eq!(integrity["scope"]["kind"], "user");
    assert!(integrity["documentCount"].as_u64().unwrap() > 0);
    assert_eq!(
        integrity["documentCount"],
        integrity["indexedDocumentCount"]
    );

    let backup_path = temp.path().join("knowledge.a3s-okf-backup");
    let backup = Command::new(binary())
        .args([
            "knowledge",
            "backup",
            backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(backup.status.success(), "{backup:?}");
    let backup_json = json(&backup);
    assert_eq!(
        backup_json["data"]["knowledge"]["backup"]["scope"]["id"],
        "user/current"
    );
    assert!(backup_path.is_file());

    let verified = Command::new(binary())
        .args([
            "knowledge",
            "verify-backup",
            backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(verified.status.success(), "{verified:?}");
    assert_eq!(json(&verified)["data"]["knowledge"]["verified"], true);

    let unconfirmed_repair = Command::new(binary())
        .args(["knowledge", "repair-search-index", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!unconfirmed_repair.status.success());
    assert_eq!(
        json(&unconfirmed_repair)["error"]["code"],
        "use.cli.invalid_usage"
    );
    let repaired = Command::new(binary())
        .args(["knowledge", "repair-search-index", "--yes", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(repaired.status.success(), "{repaired:?}");
    assert_eq!(
        json(&repaired)["data"]["knowledge"]["repair"]["after"]["storage"]["retainedProjections"],
        1
    );

    let removed = cognitive_uninstall(&home, "acme/knowledge");
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(json(&removed)["data"]["changed"], true);
    let searched = Command::new(binary())
        .args(["knowledge", "search", "package activation", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!searched.status.success(), "{searched:?}");
    assert_eq!(
        json(&searched)["error"]["code"],
        "use.okf.knowledge_unavailable"
    );
    let usage = knowledge_usage(&home);
    assert!(usage.status.success(), "{usage:?}");
    let storage = &json(&usage)["data"]["knowledge"]["storage"];
    assert_eq!(storage["retainedProjections"], 0);
    assert_eq!(storage["removedTombstones"], 1);
    assert_eq!(storage["retainedExpandedBytes"], 0);
    assert_eq!(storage["reclaimableDatabaseBytes"], 0);
}

#[test]
fn signed_okf_upgrade_atomically_switches_the_cited_capability_generation() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_okf_target(
        &temp.path().join("first"),
        "1.0.0",
        "The legacyneedle decision keeps the current package generation available.",
        &target,
    );
    let next = cognitive_okf_target(
        &temp.path().join("next"),
        "1.1.0",
        "Zero-downtime knowledge cutover selects the reviewed replacement generation.",
        &target,
    );
    let repository = TestRepository::with_targets(vec![first, next], 23, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");

    let installed = cognitive_registry_install(&server, &repository, &home, "acme/knowledge", &[]);
    assert!(installed.status.success(), "{installed:?}");
    let first_search = knowledge_search(&home, "legacyneedle");
    assert!(first_search.status.success(), "{first_search:?}");
    assert_eq!(
        json(&first_search)["data"]["knowledge"]["hits"][0]["citation"]["generation"],
        1
    );

    let upgraded =
        cognitive_registry_upgrade(&server, &repository, &home, "acme/knowledge", "1.1.0", &[]);
    assert!(upgraded.status.success(), "{upgraded:?}");
    assert_eq!(json(&upgraded)["data"]["component"]["version"], "1.1.0");
    assert_eq!(
        json(&upgraded)["data"]["packageGraph"]["replacedPackages"],
        serde_json::json!(["acme/knowledge"])
    );

    let next_search = knowledge_search(&home, "zero downtime knowledge cutover");
    assert!(next_search.status.success(), "{next_search:?}");
    let next_search = json(&next_search);
    assert_eq!(
        next_search["data"]["knowledge"]["hits"][0]["citation"]["generation"],
        2
    );
    assert_eq!(
        next_search["data"]["knowledge"]["hits"][0]["citation"]["path"],
        "concepts/package-lifecycle.md"
    );

    let old_search = knowledge_search(&home, "legacyneedle");
    assert!(old_search.status.success(), "{old_search:?}");
    assert_eq!(
        json(&old_search)["data"]["knowledge"]["hits"],
        serde_json::json!([]),
        "the current capability projection must not query the retired generation"
    );
}

fn knowledge_search(home: &std::path::Path, query: &str) -> Output {
    Command::new(binary())
        .args(["knowledge", "search", query, "--json"])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn knowledge_usage(home: &std::path::Path) -> Output {
    Command::new(binary())
        .args(["knowledge", "usage", "--json"])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn cognitive_okf_target(
    fixture_root: &std::path::Path,
    version: &str,
    decision: &str,
    target: &str,
) -> TestTarget {
    let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let package_root = fixture_root.join("package");
    copy_fixture_tree(&source, &package_root);

    let decision_path = package_root.join("okf/domain-knowledge/concepts/package-lifecycle.md");
    let original = std::fs::read_to_string(&decision_path).unwrap();
    let body_start = original.find("# Decision").unwrap();
    let frontmatter = &original[..body_start];
    std::fs::write(
        &decision_path,
        format!("{frontmatter}# Decision\n\n{decision}\n"),
    )
    .unwrap();

    let okf_root = package_root.join("okf/domain-knowledge");
    let mut files = Vec::new();
    collect_okf_files(&okf_root, &okf_root, &mut files);
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let limits = a3s_use_core::OkfBundleLimits {
        max_files: 256,
        max_concepts: 64,
        max_expanded_bytes: 67_108_864,
        max_document_bytes: 1_048_576,
        max_links_per_document: 2_048,
    };
    let inspection = a3s_use_core::inspect_okf_bundle_files(
        a3s_use_core::OkfFormatVersion::V0_2,
        limits,
        &files,
    )
    .unwrap();

    let manifest_path = package_root.join("a3s-use-extension.acl");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace(
            "version        = \"1.0.0\"",
            &format!("version        = \"{version}\""),
        )
        .replace(
            "sha256:bd85b0b63adb32bdf616384a619286af4c32401542655dd09e00450902ab478d",
            &inspection.content_digest,
        )
        .replace(
            "expanded_bytes         = 2053",
            &format!("expanded_bytes         = {}", inspection.expanded_bytes),
        );
    std::fs::write(&manifest_path, &manifest).unwrap();
    let parsed = a3s_use_extension::ExtensionManifest::parse_acl(&manifest).unwrap();
    assert_eq!(
        parsed.okf[0].bundle.content_digest,
        inspection.content_digest
    );

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.version = version.to_string();
    catalog.target = target.to_string();
    catalog.surfaces[0].okf_bundle = Some(parsed.okf[0].bundle.clone());
    catalog.archive.target_name = format!(
        "extensions/acme/knowledge/{version}/stable/{target}/acme-knowledge-{version}-{target}.tar.gz"
    );
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.package.manifest_sha256 =
        Some(format!("sha256:{:x}", Sha256::digest(manifest.as_bytes())));
    catalog.validate().unwrap();

    TestTarget {
        target_name: catalog.archive.target_name.clone(),
        custom: Some(serde_json::to_value(catalog).unwrap()),
        archive,
    }
}

fn copy_fixture_tree(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_fixture_tree(&source_path, &destination_path);
        } else {
            std::fs::copy(source_path, destination_path).unwrap();
        }
    }
}

fn collect_okf_files(
    root: &std::path::Path,
    directory: &std::path::Path,
    files: &mut Vec<a3s_use_core::OkfBundleFile>,
) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_okf_files(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            files.push(a3s_use_core::OkfBundleFile::new(
                relative,
                std::fs::read(path).unwrap(),
            ));
        }
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
