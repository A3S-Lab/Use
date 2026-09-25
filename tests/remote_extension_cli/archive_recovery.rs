use super::*;

const EXTRACTION_PAYLOAD_FILES: usize = 512;
const EXTRACTION_PAYLOAD_FILE_BYTES: usize = 8 * 1_024;

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
    let package_digest = target_package_digest(&package);
    let artifact = expanded_package_artifact(&temp.path().join("home"), &package_digest);
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
    let observation = source_target_observation(&cache_directory, &repository.target_sha256);
    let blob = raw_blob_artifact(&home, &repository.target_sha256);
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
        .env("TMPDIR", &process_temp)
        .env("TMP", &process_temp)
        .env("TEMP", &process_temp)
        .spawn()
        .unwrap();
    let reached_extraction = wait_for_partial_extraction(&process_temp);
    if !reached_extraction {
        let process_status = interrupted.try_wait().unwrap();
        let extracted_files = extraction_payload_count(&process_temp);
        let cached = observation.exists() && blob.exists();
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
    assert!(observation.is_file());
    assert!(blob.is_file());
    // Kill before package publication: no legacy authority and no installed lock.
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "operations/package-graphs").exists());
    assert!(!artifact.exists());
    if scoped_state(&home, "control.sqlite3").is_file() {
        let locks = CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home)))
            .unwrap()
            .installed_package_locks()
            .await_in_test()
            .unwrap();
        assert!(locks.is_empty());
    }

    server.clear_requests();
    let recovered =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(server.requests().is_empty());
    assert!(!partial.exists());
    assert!(observation.is_file());
    assert!(blob.is_file());
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
    assert!(artifact.is_dir());
}

#[test]
fn killed_artifact_staging_reclaims_and_replays_exact_control_install() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = skill_target_with_payload(
        temp.path(),
        &target,
        EXTRACTION_PAYLOAD_FILES,
        EXTRACTION_PAYLOAD_FILE_BYTES,
    );
    let package_digest = target_package_digest(&package);
    let repository = TestRepository::with_targets(vec![package], 97, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let package_parent = expanded_package_artifact(&home, &package_digest)
        .parent()
        .unwrap()
        .to_path_buf();

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
        .for_test_installation()
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    let reached_staging = wait_for_partial_lifecycle_staging(&package_parent);
    if !reached_staging {
        let process_status = interrupted.try_wait().unwrap();
        let staged_files = lifecycle_staging_payload_count(&package_parent);
        let _ = interrupted.kill();
        let _ = interrupted.wait();
        panic!(
            "install did not pause during Artifact Store staging: status={process_status:?}, staged_files={staged_files:?}"
        );
    }

    interrupted.kill().unwrap();
    interrupted.wait().unwrap();
    let staged_files = lifecycle_staging_payload_count(&package_parent).unwrap();
    assert!(staged_files > 0 && staged_files < EXTRACTION_PAYLOAD_FILES);
    // Staging is Artifact-owned; Control must not yet own an installed lock.
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "operations/package-graphs").exists());
    if scoped_state(&home, "control.sqlite3").is_file() {
        let locks = CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home)))
            .unwrap()
            .installed_package_locks()
            .await_in_test()
            .unwrap();
        assert!(locks.is_empty());
    }

    server.clear_requests();
    let recovered =
        cognitive_registry_install(&server, &repository, &home, "acme/root", &["--offline"]);
    assert!(recovered.status.success(), "{recovered:?}");
    assert!(server.requests().is_empty());
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());
    assert!(!scoped_state(&home, "registry.json").exists());
    assert!(std::fs::read_dir(&package_parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".artifact-staging-")
    }));
    let manager =
        CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home))).unwrap();
    let locks = manager.installed_package_locks().await_in_test().unwrap();
    assert_eq!(locks.len(), 1);
    assert_eq!(locks[0].root_package_id, "acme/root");
}

#[test]
fn uninstall_retires_control_selection_without_deleting_global_artifact() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = skill_target_with_payload(
        temp.path(),
        &target,
        EXTRACTION_PAYLOAD_FILES,
        EXTRACTION_PAYLOAD_FILE_BYTES,
    );
    let package_digest = target_package_digest(&package);
    let repository = TestRepository::with_targets(vec![package], 101, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let installed = cognitive_registry_install(&server, &repository, &home, "acme/root", &[]);
    assert!(installed.status.success(), "{installed:?}");

    let artifact = expanded_package_artifact(&home, &package_digest);
    let payload_root = artifact.join("payload");
    assert!(artifact.is_dir());
    assert_eq!(
        std::fs::read_dir(&payload_root).unwrap().count(),
        EXTRACTION_PAYLOAD_FILES
    );
    assert!(scoped_state(&home, "control.sqlite3").is_file());
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "installation-snapshot.json").exists());

    let manager =
        CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home))).unwrap();
    let before = manager
        .observe_package("acme/root")
        .await_in_test()
        .unwrap();
    assert_eq!(before.desired, PluginDesiredState::Enabled);
    let generation_before = before.package_generation.expect("installed generation");

    let removed = cognitive_uninstall(&home, "acme/root");
    assert!(removed.status.success(), "{removed:?}");
    assert!(artifact.is_dir());
    assert_eq!(
        std::fs::read_dir(&payload_root).unwrap().count(),
        EXTRACTION_PAYLOAD_FILES
    );
    assert!(!scoped_state(&home, "extensions").exists());
    assert!(!scoped_state(&home, "operations/package-graphs").exists());
    assert!(scoped_state(&home, "control.sqlite3").is_file());

    let after_manager =
        CognitivePackageManager::new(ExtensionRegistry::new(extension_paths(&home))).unwrap();
    assert!(after_manager
        .installed_package_locks()
        .await_in_test()
        .unwrap()
        .is_empty());
    let after = after_manager
        .observe_package("acme/root")
        .await_in_test()
        .unwrap();
    assert_eq!(after.desired, PluginDesiredState::Absent);
    assert!(
        after.package_generation.is_none() || after.package_generation != Some(generation_before)
    );
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
            .starts_with(".artifact-staging-")
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

trait AwaitInTest {
    type Output;
    fn await_in_test(self) -> Self::Output;
}

impl<F> AwaitInTest for F
where
    F: std::future::Future,
{
    type Output = F::Output;

    fn await_in_test(self) -> Self::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(self)
    }
}
