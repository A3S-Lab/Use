use super::*;

#[test]
fn registry_source_cli_requires_reviewed_revisions_for_authority_changes() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");

    let empty = Command::new(binary())
        .args(["registry", "source", "list", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(empty.status.success(), "{empty:?}");
    let empty = json(&empty);
    assert!(empty["data"]["registrySources"]["sources"]
        .as_array()
        .unwrap()
        .is_empty());

    let primary = Command::new(binary())
        .args([
            "registry",
            "source",
            "add",
            "primary",
            "--url",
            "https://primary.example/a3s",
            "--trust-root",
            &"a".repeat(64),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(primary.status.success(), "{primary:?}");
    let primary = json(&primary);
    let primary_revision = primary["data"]["registrySources"]["snapshot"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        primary["data"]["registrySources"]["snapshot"]["defaultRegistry"],
        "primary"
    );

    let mirror = Command::new(binary())
        .args([
            "registry",
            "source",
            "add",
            "mirror",
            "--url",
            "https://mirror.example/a3s/",
            "--trust-root",
            &"b".repeat(64),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(mirror.status.success(), "{mirror:?}");
    let mirror = json(&mirror);
    let current_revision = mirror["data"]["registrySources"]["snapshot"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();

    let stale = Command::new(binary())
        .args([
            "registry",
            "source",
            "default",
            "mirror",
            "--expected-revision",
            &primary_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!stale.status.success(), "{stale:?}");
    assert_eq!(
        json(&stale)["error"]["code"],
        "use.extension.registry_sources_revision_mismatch"
    );

    let selected = Command::new(binary())
        .args([
            "registry",
            "source",
            "default",
            "mirror",
            "--expected-revision",
            &current_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(selected.status.success(), "{selected:?}");
    let selected = json(&selected);
    let selected_revision = selected["data"]["registrySources"]["snapshot"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();

    let unconfirmed = Command::new(binary())
        .args([
            "registry",
            "source",
            "replace",
            "primary",
            "--url",
            "https://replacement.example/a3s/",
            "--trust-root",
            &"c".repeat(64),
            "--expected-revision",
            &selected_revision,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!unconfirmed.status.success(), "{unconfirmed:?}");
    assert_eq!(json(&unconfirmed)["error"]["code"], "use.cli.invalid_usage");

    let default_removal = Command::new(binary())
        .args([
            "registry",
            "source",
            "remove",
            "mirror",
            "--expected-revision",
            &selected_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!default_removal.status.success(), "{default_removal:?}");
    assert_eq!(
        json(&default_removal)["error"]["code"],
        "use.extension.registry_source_default_conflict"
    );

    let primary_default = Command::new(binary())
        .args([
            "registry",
            "source",
            "default",
            "primary",
            "--expected-revision",
            &selected_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(primary_default.status.success(), "{primary_default:?}");
    let primary_default = json(&primary_default);
    let primary_default_revision = primary_default["data"]["registrySources"]["snapshot"]
        ["revision"]
        .as_str()
        .unwrap()
        .to_owned();

    let disabled = Command::new(binary())
        .args([
            "registry",
            "source",
            "disable",
            "mirror",
            "--expected-revision",
            &primary_default_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(disabled.status.success(), "{disabled:?}");
    let disabled = json(&disabled);
    let disabled_revision = disabled["data"]["registrySources"]["snapshot"]["revision"]
        .as_str()
        .unwrap()
        .to_owned();

    let rejected = Command::new(binary())
        .args([
            "install",
            "acme/example",
            "--registry-name",
            "mirror",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!rejected.status.success(), "{rejected:?}");
    assert_eq!(
        json(&rejected)["error"]["code"],
        "use.extension.registry_source_disabled"
    );

    let enabled = Command::new(binary())
        .args([
            "registry",
            "source",
            "enable",
            "mirror",
            "--expected-revision",
            &disabled_revision,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(enabled.status.success(), "{enabled:?}");

    assert!(home.join("state/registries.acl").is_file());
    let acl = std::fs::read_to_string(home.join("state/registries.acl")).unwrap();
    assert!(acl.starts_with("registries {\n"));
    assert!(acl.contains("registry \"primary\""));
    assert!(acl.contains("registry \"mirror\""));
}

#[test]
fn registry_source_cli_fails_closed_when_another_process_owns_the_source_lock() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let _lock = exclusive_lock(&home.join("state/.registries.lock"));

    let blocked = Command::new(binary())
        .args([
            "registry",
            "source",
            "add",
            "packages",
            "--url",
            "https://registry.example/a3s/",
            "--trust-root",
            &"a".repeat(64),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();

    assert!(!blocked.status.success(), "{blocked:?}");
    assert_eq!(
        json(&blocked)["error"]["code"],
        "use.extension.registry_sources_busy"
    );
    assert!(!home.join("state/registries.acl").exists());
}

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
    let manager = &installed_json["data"]["pluginManager"];
    assert_eq!(
        manager["operationId"],
        manager["plan"]["plan"]["plan"]["operationId"]
    );
    assert_eq!(manager["planDigest"], manager["plan"]["plan"]["planDigest"]);
    assert_eq!(manager["operationId"], manager["result"]["operationId"]);
    assert_eq!(manager["planDigest"], manager["result"]["planDigest"]);
    assert_eq!(manager["plan"]["replayed"], false);
    assert_eq!(manager["result"]["replayed"], false);
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
    let lifecycle = &inspected["data"]["lifecycle"];
    assert_eq!(
        lifecycle["schema"],
        "a3s.use.plugin-lifecycle-diagnostic.v1"
    );
    assert_eq!(lifecycle["scope"]["kind"], "user");
    assert_eq!(lifecycle["scope"]["id"], "user/current");
    assert_eq!(lifecycle["packageId"], "a3s/science");
    assert_eq!(lifecycle["latest"]["status"], "completed");
    assert_eq!(
        lifecycle["latest"]["completedCheckpoints"],
        lifecycle["latest"]["totalCheckpoints"]
    );
    let checkpoints = lifecycle["latest"]["checkpoints"].as_array().unwrap();
    assert!(!checkpoints.is_empty());
    assert!(checkpoints.iter().all(|checkpoint| matches!(
        checkpoint["status"].as_str(),
        Some("applied" | "optional-failed")
    )));
    let encoded_lifecycle = serde_json::to_string(lifecycle).unwrap();
    assert!(!encoded_lifecycle.contains("idempotencyKey"));
    assert!(!encoded_lifecycle.contains("credential"));
    assert!(!encoded_lifecycle.contains("token"));

    let second = registry_install(&server, &repository, &home, Some(&package_lock_digest), &[]);
    assert!(second.status.success(), "{second:?}");
    let second = json(&second);
    assert_eq!(second["data"]["changed"], false);
    assert_eq!(second["data"]["pluginManager"]["plan"]["replayed"], true);
    assert_eq!(second["data"]["pluginManager"]["result"]["replayed"], true);
    assert!(home.join("state/plugin-host-manager").is_dir());
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
    let package = cognitive_okf_target(
        temp.path(),
        "1.0.0",
        "Package activation keeps exact-generation evidence ready.",
        &target,
    );
    let repository = TestRepository::with_targets(vec![package], 17, FUTURE);
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

    let state_backup_path = temp.path().join("use-state.a3s-use-state-backup");
    server.clear_requests();
    let state_backup = Command::new(binary())
        .args([
            "state",
            "backup",
            state_backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(state_backup.status.success(), "{state_backup:?}");
    assert!(server.requests().is_empty());
    let state_backup = json(&state_backup);
    let state_manifest = &state_backup["data"];
    assert_eq!(state_manifest["schema"], "a3s.use.state-backup.v1");
    assert_eq!(state_manifest["authority"]["registryGeneration"], 1);
    assert_eq!(
        state_manifest["authority"]["packages"][0]["packageId"],
        "acme/knowledge"
    );
    assert!(state_manifest["authority"]["packages"][0]["receiptDigest"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    let state_entries = state_manifest["entries"].as_array().unwrap();
    assert!(state_entries.iter().any(|entry| {
        entry["root"] == "data"
            && entry["path"]
                .as_str()
                .is_some_and(|path| path.starts_with("extensions/acme/knowledge/"))
    }));
    assert!(state_entries.iter().any(|entry| {
        entry["root"] == "state"
            && entry["family"] == "knowledge"
            && entry["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("knowledge.sqlite3"))
    }));
    let encoded_state_manifest = serde_json::to_string(state_manifest).unwrap();
    assert!(!encoded_state_manifest.contains(home.to_str().unwrap()));

    server.clear_requests();
    let verified_state = Command::new(binary())
        .args([
            "state",
            "verify-backup",
            state_backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", temp.path().join("offline-verifier"))
        .output()
        .unwrap();
    assert!(verified_state.status.success(), "{verified_state:?}");
    assert!(server.requests().is_empty());
    assert_eq!(json(&verified_state)["data"], *state_manifest);

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

    let retention_directory = temp.path().join("knowledge-backups");
    std::fs::create_dir(&retention_directory).unwrap();
    let first_retained = retention_directory.join("001.a3s-okf-backup");
    let second_retained = retention_directory.join("002.a3s-okf-backup");
    std::fs::copy(&backup_path, &first_retained).unwrap();
    std::fs::copy(&backup_path, &second_retained).unwrap();
    let retention_plan = Command::new(binary())
        .args([
            "knowledge",
            "backup-retention",
            retention_directory.to_str().unwrap(),
            "--max-backups",
            "1",
            "--max-bytes",
            "1073741824",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(retention_plan.status.success(), "{retention_plan:?}");
    let retention_plan = json(&retention_plan);
    let plan_digest = retention_plan["data"]["knowledge"]["backupRetention"]["planDigest"]
        .as_str()
        .unwrap();
    assert_eq!(
        retention_plan["data"]["knowledge"]["backupRetention"]["plan"]["remove"][0]["fileName"],
        "001.a3s-okf-backup"
    );

    let stale_apply = Command::new(binary())
        .args([
            "knowledge",
            "backup-retention",
            retention_directory.to_str().unwrap(),
            "--max-backups",
            "1",
            "--max-bytes",
            "1073741824",
            "--plan-digest",
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!stale_apply.status.success(), "{stale_apply:?}");
    assert_eq!(
        json(&stale_apply)["error"]["code"],
        "use.okf.knowledge_backup_retention_plan_mismatch"
    );
    assert!(first_retained.is_file());
    assert!(second_retained.is_file());

    let applied_retention = Command::new(binary())
        .args([
            "knowledge",
            "backup-retention",
            retention_directory.to_str().unwrap(),
            "--max-backups",
            "1",
            "--max-bytes",
            "1073741824",
            "--plan-digest",
            plan_digest,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(applied_retention.status.success(), "{applied_retention:?}");
    let applied_retention = json(&applied_retention);
    assert_eq!(
        applied_retention["data"]["knowledge"]["backupRetention"]["result"]["retainedBackupCount"],
        1
    );
    assert!(!first_retained.exists());
    assert!(second_retained.is_file());

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

    let scope_digest = format!("{:x}", Sha256::digest(b"user/current"));
    let binding_directory = home
        .join("state/bindings/knowledge/user")
        .join(&scope_digest)
        .join("acme/knowledge/okf-domain-knowledge");
    let binding_path = std::fs::read_dir(&binding_directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .unwrap();
    std::fs::remove_file(&binding_path).unwrap();
    let database_path = home
        .join("state/knowledge/sqlite/user")
        .join(scope_digest)
        .join("knowledge.sqlite3");
    let connection = rusqlite::Connection::open(&database_path).unwrap();
    connection
        .execute("DELETE FROM knowledge_documents_fts", [])
        .unwrap();
    drop(connection);

    let restore_plan = Command::new(binary())
        .args([
            "knowledge",
            "plan-restore",
            backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(restore_plan.status.success(), "{restore_plan:?}");
    let restore_plan = json(&restore_plan);
    assert_eq!(
        restore_plan["data"]["knowledge"]["restorePlan"]["status"],
        "required"
    );
    assert_eq!(
        restore_plan["data"]["knowledge"]["restorePlan"]["missingBindings"],
        1
    );
    assert!(
        restore_plan["data"]["knowledge"]["restorePlan"]["bindingStateDigest"]
            .as_str()
            .unwrap()
            .starts_with("sha256:")
    );
    let restore_plan_digest = restore_plan["data"]["knowledge"]["planDigest"]
        .as_str()
        .unwrap();

    let unconfirmed_restore = Command::new(binary())
        .args([
            "knowledge",
            "restore",
            backup_path.to_str().unwrap(),
            "--plan-digest",
            restore_plan_digest,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!unconfirmed_restore.status.success());
    assert_eq!(
        json(&unconfirmed_restore)["error"]["code"],
        "use.cli.invalid_usage"
    );

    let restored = Command::new(binary())
        .args([
            "knowledge",
            "restore",
            backup_path.to_str().unwrap(),
            "--plan-digest",
            restore_plan_digest,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(restored.status.success(), "{restored:?}");
    let restored = json(&restored);
    assert_eq!(restored["data"]["knowledge"]["restore"]["changed"], true);
    assert_eq!(
        restored["data"]["knowledge"]["restore"]["planDigest"],
        restore_plan_digest
    );
    assert!(
        restored["data"]["knowledge"]["restore"]["preservedPriorFiles"]
            .as_u64()
            .unwrap()
            >= 1
    );
    assert_eq!(
        restored["data"]["knowledge"]["restore"]["restoredBindings"],
        1
    );
    assert!(binding_path.is_file());

    let restore_status = Command::new(binary())
        .args(["knowledge", "restore-status", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(restore_status.status.success(), "{restore_status:?}");
    let restore_status = json(&restore_status);
    let diagnostic = &restore_status["data"]["knowledge"]["restoreStatus"];
    assert_eq!(
        diagnostic["schema"],
        "a3s.use.okf-knowledge-restore-diagnostic.v2"
    );
    assert!(diagnostic["active"].is_null());
    assert_eq!(diagnostic["retainedOperationDirectories"], 1);
    assert_eq!(diagnostic["retentionLimit"], 32);
    assert_eq!(diagnostic["retentionRemaining"], 31);
    assert_eq!(diagnostic["operations"][0]["status"], "completed");
    assert_eq!(diagnostic["operations"][0]["missingBindings"], 1);
    assert_eq!(
        diagnostic["operations"][0]["planDigest"],
        restore_plan_digest
    );

    let audited = Command::new(binary())
        .args(["knowledge", "audit", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(audited.status.success(), "{audited:?}");

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
