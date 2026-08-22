use super::*;

const EXTRACTION_PAYLOAD_FILES: usize = 512;
const EXTRACTION_PAYLOAD_FILE_BYTES: usize = 8 * 1_024;
const REMOVAL_PAYLOAD_FILES: usize = 2_048;

#[test]
fn killed_registry_archive_extraction_retries_offline_without_partial_publication() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = skill_target_with_payload(
        temp.path(),
        &target,
        EXTRACTION_PAYLOAD_FILES,
        EXTRACTION_PAYLOAD_FILE_BYTES,
    );
    let repository = TestRepository::with_targets(vec![package], 89, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let process_temp = temp.path().join("process-temp");
    std::fs::create_dir_all(&process_temp).unwrap();

    configure_registry(&server, &repository, &home, &[]);
    let source = registry_source_snapshot(&home)["sources"][0].clone();
    let source_identity = source["sourceIdentity"].as_str().unwrap();
    let cache_directory = home
        .join("state/remote-registries/fixture/sources")
        .join(source_identity)
        .join("verified-targets/sha256");
    let partial = cache_directory.join(format!(".target-{}.part", repository.target_sha256));
    let verified = cache_directory.join(&repository.target_sha256);
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
        .env("TMPDIR", &process_temp)
        .env("TMP", &process_temp)
        .env("TEMP", &process_temp)
        .spawn()
        .unwrap();
    let reached_extraction = wait_for_partial_extraction(&process_temp);
    if !reached_extraction {
        let process_status = interrupted.try_wait().unwrap();
        let extracted_files = extraction_payload_count(&process_temp);
        let cached = verified.exists();
        let requests = server.requests();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        panic!(
            "install did not pause during Registry archive extraction: status={process_status:?}, extracted_files={extracted_files:?}, cached={cached}, requests={requests:?}"
        );
    }

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    let extracted_files = extraction_payload_count(&process_temp).unwrap();
    assert!(extracted_files > 0 && extracted_files < EXTRACTION_PAYLOAD_FILES);
    assert!(!partial.exists());
    assert!(verified.is_file());
    assert!(!home.join("state/extensions/acme/root.json").exists());
    assert!(!home.join("state/package-graphs/acme/root.json").exists());
    assert!(!home
        .join("state/operations/package-graphs/install/acme/root.json")
        .exists());
    assert!(!home.join("data/extensions/acme/root").exists());

    server.clear_requests();
    let recovered =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(server.requests().is_empty());
    assert!(!partial.exists());
    assert!(verified.is_file());
    assert!(home.join("state/extensions/acme/root.json").is_file());
    assert!(home.join("state/package-graphs/acme/root.json").is_file());
    assert!(home.join("data/extensions/acme/root").is_dir());
}

#[test]
fn killed_lifecycle_package_copy_reclaims_staging_and_replays_exact_install() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = skill_target_with_payload(
        temp.path(),
        &target,
        EXTRACTION_PAYLOAD_FILES,
        EXTRACTION_PAYLOAD_FILE_BYTES,
    );
    let repository = TestRepository::with_targets(vec![package], 97, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let package_parent = home.join("data/extensions/acme/root");
    let pending_path = home.join("state/operations/package-graphs/install/acme/root.json");
    let receipt_path = home.join("state/extensions/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");
    let lifecycle_path = lifecycle_journal_path(&home, "acme/root");

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
    let reached_staging = wait_for_partial_lifecycle_staging(&package_parent);
    if !reached_staging {
        let process_status = interrupted.try_wait().unwrap();
        let staged_files = lifecycle_staging_payload_count(&package_parent);
        let pending = std::fs::read_to_string(&pending_path).ok();
        let lifecycle = std::fs::read_to_string(&lifecycle_path).ok();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        panic!(
            "install did not pause during lifecycle package copy: status={process_status:?}, staged_files={staged_files:?}, pending={pending:?}, lifecycle={lifecycle:?}"
        );
    }

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    let staged_files = lifecycle_staging_payload_count(&package_parent).unwrap();
    assert!(staged_files > 0 && staged_files < EXTRACTION_PAYLOAD_FILES);
    assert!(pending_path.is_file());
    assert!(!receipt_path.exists());
    assert!(!graph_path.exists());
    let lifecycle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lifecycle_path).unwrap()).unwrap();
    assert_eq!(lifecycle["status"], "applying");
    assert!(lifecycle["receipts"].as_array().is_none_or(Vec::is_empty));
    let snapshot_path = home.join("state/registry.json");
    if snapshot_path.exists() {
        let snapshot: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
        assert!(snapshot["routes"].as_array().unwrap().is_empty());
    }

    server.clear_requests();
    let recovered =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(server.requests().is_empty());
    assert!(!pending_path.exists());
    assert!(receipt_path.is_file());
    assert!(graph_path.is_file());
    assert!(std::fs::read_dir(&package_parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".lifecycle-staging-")
    }));
    let lifecycle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lifecycle_path).unwrap()).unwrap();
    assert_eq!(lifecycle["status"], "completed");
    assert_eq!(
        lifecycle["receipts"].as_array().unwrap().len(),
        lifecycle["intent"]["checkpoints"].as_array().unwrap().len()
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(snapshot_path).unwrap()).unwrap();
    assert_eq!(snapshot["generation"], 1);
    assert!(snapshot["routes"]
        .as_array()
        .unwrap()
        .iter()
        .any(|route| { route["packageId"] == "acme/root" && route["lifecycleGeneration"] == 1 }));
}

#[test]
fn killed_package_removal_replays_partial_directory_without_generation_inflation() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = skill_target_with_payload(temp.path(), &target, REMOVAL_PAYLOAD_FILES, 1);
    let repository = TestRepository::with_targets(vec![package], 101, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let receipt_path = home.join("state/extensions/acme/root.json");
    let receipt: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&receipt_path).unwrap()).unwrap();
    let generation = receipt["lifecycleGeneration"].as_u64().unwrap();
    let package_sha256 = receipt["packageSha256"].as_str().unwrap();
    let package_root = std::path::PathBuf::from(receipt["packageRoot"].as_str().unwrap());
    let payload_root = package_root.join("payload");
    assert_eq!(
        std::fs::read_dir(&payload_root).unwrap().count(),
        REMOVAL_PAYLOAD_FILES
    );
    let retained_receipt = home
        .join("state/extension-generations/acme/root")
        .join(format!("{generation:020}-{package_sha256}.json"));
    let pending_path = home.join("state/operations/package-graphs/uninstall/acme/root.json");
    let graph_path = home.join("state/package-graphs/acme/root.json");
    let snapshot_path = home.join("state/registry.json");
    let lifecycle_path = lifecycle_journal_path(&home, "acme/root");
    let generation_before =
        serde_json::from_slice::<serde_json::Value>(&std::fs::read(&snapshot_path).unwrap())
            .unwrap()["generation"]
            .as_u64()
            .unwrap();

    let mut interrupted = Command::new(binary())
        .args(["uninstall", "acme/root", "--json"])
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_removal = wait_for_partial_package_removal(&payload_root);
    if !reached_removal {
        let process_status = interrupted.try_wait().unwrap();
        let remaining_files = directory_entry_count(&payload_root);
        let pending = std::fs::read_to_string(&pending_path).ok();
        let lifecycle = std::fs::read_to_string(&lifecycle_path).ok();
        let snapshot = std::fs::read_to_string(&snapshot_path).ok();
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        panic!(
            "uninstall did not pause during physical package removal: status={process_status:?}, remaining_files={remaining_files:?}, pending={pending:?}, lifecycle={lifecycle:?}, snapshot={snapshot:?}"
        );
    }

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    let remaining_files = directory_entry_count(&payload_root).unwrap();
    assert!(remaining_files > 0 && remaining_files < REMOVAL_PAYLOAD_FILES);
    assert!(pending_path.is_file());
    assert!(graph_path.is_file());
    assert!(!receipt_path.exists());
    assert!(!retained_receipt.exists());
    let lifecycle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lifecycle_path).unwrap()).unwrap();
    assert_eq!(lifecycle["status"], "applying");
    assert_eq!(
        lifecycle["receipts"].as_array().unwrap().len() + 1,
        lifecycle["intent"]["checkpoints"].as_array().unwrap().len()
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&snapshot_path).unwrap()).unwrap();
    assert_eq!(snapshot["generation"], generation_before + 1);
    assert!(snapshot["routes"]
        .as_array()
        .unwrap()
        .iter()
        .all(|route| route["packageId"] != "acme/root"));
    assert_eq!(snapshot["pendingCutovers"].as_array().unwrap().len(), 1);

    let recovered = cognitive_uninstall(&home, "acme/root");
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(!package_root.exists());
    assert!(!pending_path.exists());
    assert!(!graph_path.exists());
    let lifecycle: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&lifecycle_path).unwrap()).unwrap();
    assert_eq!(lifecycle["status"], "completed");
    assert_eq!(
        lifecycle["receipts"].as_array().unwrap().len(),
        lifecycle["intent"]["checkpoints"].as_array().unwrap().len()
    );
    let snapshot: serde_json::Value =
        serde_json::from_slice(&std::fs::read(snapshot_path).unwrap()).unwrap();
    assert_eq!(snapshot["generation"], generation_before + 1);
    assert!(snapshot["pendingCutovers"]
        .as_array()
        .is_none_or(Vec::is_empty));
}

fn skill_target_with_payload(
    fixture_root: &std::path::Path,
    target: &str,
    file_count: usize,
    file_bytes: usize,
) -> TestTarget {
    let mut package = cognitive_skill_target(fixture_root, "acme/root", "root", Vec::new(), target);
    let package_root = fixture_root.join("packages/root");
    let payload_root = package_root.join("payload");
    std::fs::create_dir_all(&payload_root).unwrap();
    let payload = vec![0x5a; file_bytes];
    for index in 0..file_count {
        std::fs::write(payload_root.join(format!("{index:04}.bin")), &payload).unwrap();
    }

    package.archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let mut catalog: PluginCatalogRecord =
        serde_json::from_value(package.custom.take().unwrap()).unwrap();
    catalog.archive.length = package.archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&package.archive));
    catalog.package.file_count = fingerprint.1;
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.validate().unwrap();
    package.target_name = catalog.archive.target_name.clone();
    package.custom = Some(serde_json::to_value(catalog).unwrap());
    package
}

fn extraction_payload_count(temporary_root: &std::path::Path) -> Option<usize> {
    let temporary_directories = std::fs::read_dir(temporary_root).ok()?;
    for temporary in temporary_directories.flatten() {
        let payload = temporary.path().join("package/package/payload");
        if let Ok(entries) = std::fs::read_dir(payload) {
            return Some(entries.filter_map(Result::ok).count());
        }
    }
    None
}

fn wait_for_partial_extraction(temporary_root: &std::path::Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if extraction_payload_count(temporary_root)
            .is_some_and(|count| count > 0 && count < EXTRACTION_PAYLOAD_FILES)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

fn lifecycle_staging_payload_count(package_parent: &std::path::Path) -> Option<usize> {
    let entries = std::fs::read_dir(package_parent).ok()?;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with(".lifecycle-staging-")
        {
            continue;
        }
        if let Ok(payload) = std::fs::read_dir(entry.path().join("payload")) {
            return Some(payload.filter_map(Result::ok).count());
        }
    }
    None
}

fn wait_for_partial_lifecycle_staging(package_parent: &std::path::Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if lifecycle_staging_payload_count(package_parent)
            .is_some_and(|count| count > 0 && count < EXTRACTION_PAYLOAD_FILES)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}

fn directory_entry_count(directory: &std::path::Path) -> Option<usize> {
    std::fs::read_dir(directory)
        .ok()
        .map(|entries| entries.filter_map(Result::ok).count())
}

fn wait_for_partial_package_removal(payload_root: &std::path::Path) -> bool {
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        if directory_entry_count(payload_root)
            .is_some_and(|count| count > 0 && count < REMOVAL_PAYLOAD_FILES)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    false
}
