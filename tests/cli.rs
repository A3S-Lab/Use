#[cfg(all(feature = "extensions", unix))]
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(feature = "ocr")]
use std::path::Path;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
}

#[test]
fn capabilities_are_available_as_versioned_json() {
    let output = Command::new(binary())
        .args(["capabilities", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["schemaVersion"], 1);
    let domains = value["data"]["domains"].as_array().unwrap();
    for id in ["browser", "ocr", "box"] {
        assert!(
            domains.iter().any(|domain| domain["id"] == id),
            "missing built-in domain {id}: {domains:?}"
        );
    }
    assert!(value["data"].get("customJsonRpc").is_none());
    assert!(value.get("jsonrpc").is_none());
}

#[test]
fn unified_capability_snapshot_projects_builtin_skills() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_HOME", temp.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let capabilities = value["data"]["registry"]["capabilities"]
        .as_array()
        .unwrap();
    let browser = capabilities
        .iter()
        .find(|capability| capability["id"] == "use/browser")
        .unwrap();
    assert_eq!(browser["origin"], "built-in");
    #[cfg(feature = "browser")]
    {
        assert!(browser["skills"][0]["path"].as_str().is_some_and(|path| {
            std::path::Path::new(path).ends_with(
                std::path::Path::new("skills")
                    .join("a3s-use-browser")
                    .join("SKILL.md"),
            )
        }));
        let skill_digest = browser["skills"][0]["sha256"]
            .as_str()
            .expect("the capability registry must bind Skill content, not only its path");
        assert_eq!(skill_digest.len(), 64);
        assert!(skill_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    }
    #[cfg(not(feature = "browser"))]
    {
        assert_eq!(browser["enabled"], false);
        assert_eq!(browser["surfaces"], serde_json::json!([]));
        assert!(browser.get("skills").is_none());
    }
    assert!(capabilities
        .iter()
        .all(|capability| capability["route"] != "office"));
    assert!(value.get("jsonrpc").is_none());
}

#[test]
fn capability_watch_uses_revision_to_report_an_unchanged_snapshot() {
    let temp = tempfile::tempdir().unwrap();
    let snapshot = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_HOME", temp.path())
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot: serde_json::Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    let registry = &snapshot["data"]["registry"];
    let generation = registry["generation"].as_u64().unwrap().to_string();
    let revision = registry["revision"].as_str().unwrap();

    let watched = Command::new(binary())
        .args([
            "capability",
            "watch",
            "--after-generation",
            &generation,
            "--after-revision",
            revision,
            "--timeout-ms",
            "1",
            "--json",
        ])
        .env("A3S_USE_HOME", temp.path())
        .output()
        .unwrap();
    assert!(watched.status.success(), "{watched:?}");
    let watched: serde_json::Value = serde_json::from_slice(&watched.stdout).unwrap();
    assert_eq!(watched["data"]["changed"], false);
}

#[cfg(unix)]
#[test]
fn box_route_preserves_native_arguments_output_and_status() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("a3s-box-fixture");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$*\"\nexit 9\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let output = Command::new(binary())
        .args(["box", "compose", "up", "--detach"])
        .env("A3S_USE_BOX_EXECUTABLE", &executable)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(9));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "compose up --detach\n"
    );
    assert!(output.stderr.is_empty());
}

#[test]
fn box_route_fails_closed_without_an_explicit_component_path() {
    let output = Command::new(binary())
        .args(["box", "ps", "--json"])
        .env_remove("A3S_USE_BOX_EXECUTABLE")
        .output()
        .unwrap();
    assert!(!output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "use.box.missing");
}

#[test]
fn delegated_component_status_matches_the_root_cli_contract() {
    let output = Command::new(binary())
        .args(["component", "status", "browser", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["component"]["id"], "browser");
    assert!(value["component"]["presence"].is_string());
    assert!(value["component"]["health"].is_string());
}

#[test]
fn machine_errors_are_single_json_documents() {
    let output = Command::new(binary())
        .args(["unknown", "--json"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["ok"], false);
    assert_eq!(value["error"]["code"], "use.route_unknown");
}

#[cfg(feature = "extensions")]
#[test]
fn knowledge_search_fails_closed_without_an_active_projection() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["knowledge", "search", "package activation", "--json"])
        .env("A3S_USE_HOME", temporary.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "use.okf.knowledge_unavailable");
}

#[cfg(feature = "extensions")]
#[test]
fn coordinated_state_backup_cli_creates_and_verifies_offline() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let package_file = home.join("data/extensions/acme/tool/package/bin/tool");
    std::fs::create_dir_all(package_file.parent().unwrap()).unwrap();
    std::fs::write(&package_file, b"portable CLI backup fixture").unwrap();
    let backup_path = temporary.path().join("state.a3s-use-state-backup");

    let backup = Command::new(binary())
        .args(["state", "backup", backup_path.to_str().unwrap(), "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(backup.status.success(), "{backup:?}");
    assert!(backup.stderr.is_empty());
    let backup_json: serde_json::Value = serde_json::from_slice(&backup.stdout).unwrap();
    assert_eq!(backup_json["data"]["schema"], "a3s.use.state-backup.v1");
    assert_eq!(backup_json["data"]["fileCount"], 1);
    assert_eq!(backup_json["data"]["entries"][0]["root"], "data");
    assert_eq!(
        backup_json["data"]["entries"][0]["path"],
        "extensions/acme/tool/package/bin/tool"
    );
    assert!(backup_path.is_file());

    let offline_home = temporary.path().join("unrelated-empty-home");
    let verified = Command::new(binary())
        .args([
            "state",
            "verify-backup",
            backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", offline_home)
        .output()
        .unwrap();
    assert!(verified.status.success(), "{verified:?}");
    assert!(verified.stderr.is_empty());
    let verified_json: serde_json::Value = serde_json::from_slice(&verified.stdout).unwrap();
    assert_eq!(verified_json["data"], backup_json["data"]);
}

#[cfg(feature = "extensions")]
#[test]
fn coordinated_state_backup_retention_cli_requires_exact_review() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let state_file = home.join("state/registry-trust-roots/sha256/root.json");
    std::fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    let backup_directory = temporary.path().join("backups");
    std::fs::create_dir(&backup_directory).unwrap();

    for (name, bytes) in [
        ("001.a3s-use-state-backup", b"one".as_slice()),
        ("002.a3s-use-state-backup", b"two".as_slice()),
        ("003.a3s-use-state-backup", b"three".as_slice()),
    ] {
        std::fs::write(&state_file, bytes).unwrap();
        let output = Command::new(binary())
            .args([
                "state",
                "backup",
                backup_directory.join(name).to_str().unwrap(),
                "--json",
            ])
            .env("A3S_USE_HOME", &home)
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
    }

    let plan = Command::new(binary())
        .args([
            "state",
            "backup-retention",
            backup_directory.to_str().unwrap(),
            "--max-backups",
            "2",
            "--max-bytes",
            "1073741824",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(plan.status.success(), "{plan:?}");
    assert!(plan.stderr.is_empty());
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let retention = &plan["data"]["state"]["backupRetention"];
    assert_eq!(
        retention["plan"]["remove"][0]["fileName"],
        "001.a3s-use-state-backup"
    );
    assert_eq!(retention["plan"]["retainedBackupCount"], 2);
    let plan_digest = retention["planDigest"].as_str().unwrap();

    let applied = Command::new(binary())
        .args([
            "state",
            "backup-retention",
            backup_directory.to_str().unwrap(),
            "--max-backups",
            "2",
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
    assert!(applied.status.success(), "{applied:?}");
    assert!(applied.stderr.is_empty());
    let applied: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    let result = &applied["data"]["state"]["backupRetention"]["result"];
    assert_eq!(result["schema"], "a3s.use.state-backup-retention-result.v1");
    assert_eq!(result["removed"].as_array().unwrap().len(), 1);
    assert_eq!(result["retainedBackupCount"], 2);
    assert!(!backup_directory.join("001.a3s-use-state-backup").exists());
    assert!(backup_directory.join("002.a3s-use-state-backup").is_file());
    assert!(backup_directory.join("003.a3s-use-state-backup").is_file());
}

#[cfg(feature = "extensions")]
#[test]
fn coordinated_state_restore_cli_requires_review_rollback_and_confirmation() {
    let temporary = tempfile::tempdir().unwrap();
    let home = temporary.path().join("home");
    let state_file = home.join("state/knowledge/value.bin");
    std::fs::create_dir_all(state_file.parent().unwrap()).unwrap();
    std::fs::write(&state_file, b"candidate").unwrap();
    let backup_path = temporary.path().join("candidate.a3s-use-state-backup");
    let backup = Command::new(binary())
        .args(["state", "backup", backup_path.to_str().unwrap(), "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(backup.status.success(), "{backup:?}");
    std::fs::write(&state_file, b"live drift").unwrap();

    let plan = Command::new(binary())
        .args([
            "state",
            "plan-restore",
            backup_path.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(plan.status.success(), "{plan:?}");
    let plan: serde_json::Value = serde_json::from_slice(&plan.stdout).unwrap();
    let restore = &plan["data"]["state"]["restore"];
    assert_eq!(restore["plan"]["schema"], "a3s.use.state-restore-plan.v1");
    assert_eq!(restore["plan"]["summary"]["replaceFiles"], 1);
    let plan_digest = restore["planDigest"].as_str().unwrap();
    let rollback_path = temporary.path().join("rollback.a3s-use-state-backup");

    let unconfirmed = Command::new(binary())
        .args([
            "state",
            "restore",
            backup_path.to_str().unwrap(),
            "--rollback-backup",
            rollback_path.to_str().unwrap(),
            "--plan-digest",
            plan_digest,
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(!unconfirmed.status.success());
    let unconfirmed: serde_json::Value = serde_json::from_slice(&unconfirmed.stdout).unwrap();
    assert_eq!(unconfirmed["error"]["code"], "use.cli.invalid_usage");
    assert!(!rollback_path.exists());

    let applied = Command::new(binary())
        .args([
            "state",
            "restore",
            backup_path.to_str().unwrap(),
            "--rollback-backup",
            rollback_path.to_str().unwrap(),
            "--plan-digest",
            plan_digest,
            "--yes",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(applied.status.success(), "{applied:?}");
    let applied: serde_json::Value = serde_json::from_slice(&applied.stdout).unwrap();
    let result = &applied["data"]["state"]["restore"]["result"];
    assert_eq!(result["schema"], "a3s.use.state-restore-result.v1");
    assert_eq!(result["changed"], true);
    assert_eq!(result["planDigest"], plan_digest);
    assert_eq!(std::fs::read(&state_file).unwrap(), b"candidate");
    assert!(rollback_path.is_file());

    let status = Command::new(binary())
        .args(["state", "restore-status", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    let diagnostic = &status["data"]["state"]["restore"]["diagnostic"];
    assert_eq!(diagnostic["schema"], "a3s.use.state-restore-diagnostic.v1");
    assert!(diagnostic["active"].is_null());
    assert_eq!(diagnostic["operations"][0]["status"], "completed");
    assert_eq!(diagnostic["operations"][0]["planDigest"], plan_digest);
}

#[cfg(feature = "extensions")]
#[test]
fn knowledge_usage_reports_an_empty_exact_default_scope() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["knowledge", "usage", "--json"])
        .env("A3S_USE_HOME", temporary.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let storage = &value["data"]["knowledge"]["storage"];
    assert_eq!(storage["scope"]["kind"], "user");
    assert_eq!(storage["scope"]["id"], "user/current");
    assert_eq!(storage["retainedProjections"], 0);
    assert_eq!(storage["removedTombstones"], 0);
    assert_eq!(storage["retainedExpandedBytes"], 0);
    assert_eq!(storage["maxScopeProjections"], 256);
    assert_eq!(storage["maxScopeExpandedBytes"], 512 * 1024 * 1024);
    assert_eq!(storage["maxSurfaceGenerations"], 32);
    assert_eq!(storage["maxScopeTombstones"], 256);
    assert_eq!(storage["reclaimableDatabaseBytes"], 0);
}

#[cfg(feature = "extensions")]
#[test]
fn knowledge_usage_reports_an_empty_exact_workspace_scope() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args([
            "knowledge",
            "usage",
            "--scope-kind",
            "workspace",
            "--scope-id",
            "workspace/acme-project",
            "--json",
        ])
        .env("A3S_USE_HOME", temporary.path())
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let storage = &value["data"]["knowledge"]["storage"];
    assert_eq!(storage["scope"]["kind"], "workspace");
    assert_eq!(storage["scope"]["id"], "workspace/acme-project");
    assert_eq!(storage["retainedProjections"], 0);
    assert_eq!(storage["removedTombstones"], 0);
    assert_eq!(storage["retainedExpandedBytes"], 0);
}

#[cfg(feature = "extensions")]
#[test]
fn knowledge_usage_rejects_an_unknown_scope_kind() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args([
            "knowledge",
            "usage",
            "--scope-kind",
            "organization",
            "--json",
        ])
        .env("A3S_USE_HOME", temporary.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "use.cli.invalid_usage");
}

#[cfg(feature = "extensions")]
#[test]
fn knowledge_usage_requires_an_exact_workspace_scope_id() {
    let temporary = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["knowledge", "usage", "--scope-kind", "workspace", "--json"])
        .env("A3S_USE_HOME", temporary.path())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["error"]["code"], "use.cli.invalid_usage");
}

#[cfg(feature = "browser")]
#[test]
fn browser_help_succeeds_and_render_rejects_an_option_as_a_value() {
    let help = Command::new(binary())
        .args(["browser", "render", "--help"])
        .output()
        .unwrap();
    assert!(help.status.success(), "{help:?}");
    assert!(String::from_utf8(help.stdout)
        .unwrap()
        .contains("browser render <url>"));

    let invalid = Command::new(binary())
        .args([
            "browser",
            "render",
            "https://example.com",
            "--output",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(!invalid.status.success());
    assert!(invalid.stderr.is_empty());
    let value: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(value["error"]["code"], "use.cli.invalid_usage");
}

#[test]
fn mcp_stop_is_safe_when_no_service_is_running() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["mcp", "stop", "--json"])
        .env("A3S_USE_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["protocol"], "mcp-streamable-http");
    assert_eq!(value["data"]["running"], false);
}

#[cfg(all(feature = "browser", feature = "mcp"))]
struct PersistentServiceGuard {
    runtime_dir: std::path::PathBuf,
    armed: bool,
}

#[cfg(all(feature = "browser", feature = "mcp"))]
impl PersistentServiceGuard {
    fn disarm(&mut self) {
        self.armed = false;
    }
}

#[cfg(all(feature = "browser", feature = "mcp"))]
impl Drop for PersistentServiceGuard {
    fn drop(&mut self) {
        use std::process::Stdio;

        if !self.armed {
            return;
        }
        // Panic cleanup must never replace the original failure with another
        // indefinite wait. Normal test paths stop the service explicitly.
        let _ = Command::new(binary())
            .args(["mcp", "stop", "--json"])
            .env("A3S_USE_RUNTIME_DIR", &self.runtime_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(all(feature = "browser", feature = "mcp"))]
#[test]
fn browser_capability_projection_coexists_with_authenticated_standard_mcp() {
    let temp = tempfile::tempdir().unwrap();
    let mut guard = PersistentServiceGuard {
        runtime_dir: temp.path().to_path_buf(),
        armed: true,
    };

    let started = Command::new(binary())
        .args(["mcp", "start", "browser", "--json"])
        .env("A3S_USE_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(started.status.success(), "{started:?}");
    let started_json: serde_json::Value = serde_json::from_slice(&started.stdout).unwrap();
    assert_eq!(started_json["data"]["running"], true);
    assert_eq!(started_json["data"]["protocol"], "mcp-streamable-http");
    assert!(started_json["data"].get("token").is_none());
    let receipt = temp.path().join("browser-mcp.json");
    assert!(receipt.is_file());
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&receipt).unwrap().permissions().mode() & 0o777,
        0o600
    );

    let snapshot = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot_json: serde_json::Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    assert!(snapshot_json["data"]["registry"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .any(|capability| capability["id"] == "use/browser"));

    let status = Command::new(binary())
        .args(["mcp", "status", "browser", "--json"])
        .env("A3S_USE_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(status.status.success(), "{status:?}");
    let status_json: serde_json::Value = serde_json::from_slice(&status.stdout).unwrap();
    assert_eq!(status_json["data"]["running"], true);

    let stopped = Command::new(binary())
        .args(["mcp", "stop", "browser", "--json"])
        .env("A3S_USE_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(stopped.status.success(), "{stopped:?}");
    let stopped_json: serde_json::Value = serde_json::from_slice(&stopped.stdout).unwrap();
    assert_eq!(stopped_json["data"]["stopped"], true);
    assert!(!receipt.exists());
    guard.disarm();
}

#[cfg(all(unix, feature = "browser", feature = "mcp"))]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_session_state_survives_separate_cli_invocations_with_an_external_driver() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const CLI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
    const FIXTURE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

    // The independent Browser repository owns the driver and its regular E2E
    // coverage. Keep this cross-repository facade check opt-in so Use never
    // discovers an unrelated executable from a developer or runner PATH.
    let Some(driver) = std::env::var_os("A3S_USE_BROWSER_DRIVER_E2E").map(std::path::PathBuf::from)
    else {
        return;
    };
    assert!(
        driver.is_file(),
        "A3S_USE_BROWSER_DRIVER_E2E must point to the external Browser driver"
    );
    let Some(chrome) = a3s_use_browser::detect_chrome() else {
        return;
    };
    let watchdog_done = Arc::new(AtomicBool::new(false));
    let watchdog_stage = Arc::new(Mutex::new("setup"));
    {
        let done = Arc::clone(&watchdog_done);
        let stage = Arc::clone(&watchdog_stage);
        std::thread::spawn(move || {
            for _ in 0..120 {
                std::thread::sleep(std::time::Duration::from_secs(1));
                if done.load(Ordering::Acquire) {
                    return;
                }
            }
            let stage = *stage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            eprintln!("persistent Browser CLI test exceeded 120 seconds during {stage}");
            std::process::exit(124);
        });
    }
    #[cfg(unix)]
    let temp = tempfile::Builder::new()
        .prefix("a3s-")
        .tempdir_in("/tmp")
        .unwrap();
    #[cfg(not(unix))]
    let temp = tempfile::tempdir().unwrap();
    let mut guard = PersistentServiceGuard {
        runtime_dir: temp.path().to_path_buf(),
        armed: true,
    };
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (fixture_shutdown, mut fixture_shutdown_rx) = tokio::sync::oneshot::channel::<()>();
    let fixture = tokio::spawn(async move {
        let mut connections = Vec::new();
        loop {
            tokio::select! {
                _ = &mut fixture_shutdown_rx => break,
                accepted = listener.accept() => {
                    let (mut stream, _) = accepted.unwrap();
                    connections.push(tokio::spawn(async move {
                        let mut request = vec![0; 4_096];
                        let Ok(Ok(read)) = tokio::time::timeout(
                            FIXTURE_READ_TIMEOUT,
                            stream.read(&mut request),
                        )
                        .await
                        else {
                            return;
                        };
                        if read == 0 {
                            return;
                        }
                        let body = r#"<!doctype html><html><head><title>CLI session fixture</title></head><body><input id="query" aria-label="Query"></body></html>"#;
                        let response = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                            body.len()
                        );
                        let _ = stream.write_all(response.as_bytes()).await;
                        let _ = stream.shutdown().await;
                    }));
                }
            }
        }
        for connection in connections {
            connection.abort();
            let _ = connection.await;
        }
    });
    let run = |stage: &'static str, args: Vec<String>| {
        let runtime_dir = temp.path().to_path_buf();
        let chrome = chrome.clone();
        let driver = driver.clone();
        let watchdog_stage = Arc::clone(&watchdog_stage);
        async move {
            *watchdog_stage
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = stage;
            eprintln!("starting persistent Browser CLI stage: {stage}");
            let mut command = tokio::process::Command::new(binary());
            command
                .args(&args)
                .env("A3S_USE_RUNTIME_DIR", runtime_dir)
                .env("A3S_BROWSER_EXECUTABLE", chrome)
                .env("A3S_USE_BROWSER_DRIVER", driver)
                .kill_on_drop(true);
            let output = tokio::time::timeout(CLI_TIMEOUT, command.output())
                .await
                .unwrap_or_else(|_| panic!("CLI command timed out after 45 seconds: {args:?}"))
                .unwrap();
            eprintln!("completed persistent Browser CLI stage: {stage}");
            output
        }
    };

    let opened = run(
        "open",
        vec![
            "browser".into(),
            "open".into(),
            format!("http://{address}/fixture"),
            "--session".into(),
            "s".into(),
            "--wait".into(),
            "load".into(),
            "--json".into(),
        ],
    )
    .await;
    assert!(opened.status.success(), "{opened:?}");

    // `open` may omit its convenience snapshot when a backend reports load
    // completion before the accessibility tree is ready. Ask for the
    // snapshot explicitly so this test verifies the cross-process session
    // contract instead of depending on that optional response field.
    let initial_snapshot = run(
        "initial snapshot",
        vec![
            "browser".into(),
            "snapshot".into(),
            "--session".into(),
            "s".into(),
            "--json".into(),
        ],
    )
    .await;
    assert!(initial_snapshot.status.success(), "{initial_snapshot:?}");
    let initial_snapshot_json: serde_json::Value =
        serde_json::from_slice(&initial_snapshot.stdout).unwrap();
    let reference = initial_snapshot_json["data"]["refs"]
        .as_object()
        .unwrap_or_else(|| panic!("snapshot did not contain refs: {initial_snapshot_json}"))
        .iter()
        .find(|(_, element)| element["role"] == "textbox")
        .map(|(reference, _)| format!("@{reference}"))
        .unwrap_or_else(|| panic!("snapshot did not contain a textbox: {initial_snapshot_json}"));

    let typed = run(
        "type",
        vec![
            "browser".into(),
            "type".into(),
            reference,
            "persistent value".into(),
            "--session".into(),
            "s".into(),
            "--json".into(),
        ],
    )
    .await;
    assert!(typed.status.success(), "{typed:?}");

    let snapshot = run(
        "final snapshot",
        vec![
            "browser".into(),
            "snapshot".into(),
            "--session".into(),
            "s".into(),
            "--json".into(),
        ],
    )
    .await;
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot_json: serde_json::Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    assert!(snapshot_json["data"]["snapshot"]
        .as_str()
        .is_some_and(|snapshot| snapshot.contains("persistent value")));

    let closed = run(
        "close",
        vec![
            "browser".into(),
            "close".into(),
            "--session".into(),
            "s".into(),
            "--json".into(),
        ],
    )
    .await;
    assert!(closed.status.success(), "{closed:?}");

    let stopped = run(
        "stop",
        vec![
            "mcp".into(),
            "stop".into(),
            "browser".into(),
            "--json".into(),
        ],
    )
    .await;
    assert!(stopped.status.success(), "{stopped:?}");
    guard.disarm();
    let _ = fixture_shutdown.send(());
    fixture.await.unwrap();
    watchdog_done.store(true, Ordering::Release);
}

#[cfg(all(feature = "browser", feature = "mcp"))]
#[test]
fn browser_mcp_delegation_rejects_a_missing_explicit_driver() {
    let temp = tempfile::tempdir().unwrap();
    let output = Command::new(binary())
        .args(["mcp", "serve", "browser", "--tools", "all"])
        .env(
            "A3S_USE_BROWSER_DRIVER",
            temp.path().join("missing-browser-driver"),
        )
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("A3S_USE_BROWSER_DRIVER"));
    assert!(stderr.contains("does not point to a usable Browser driver"));
}

#[cfg(all(unix, feature = "browser"))]
#[test]
fn browser_install_reuses_an_explicit_provider_without_downloading() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("chrome-fixture");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let output = Command::new(binary())
        .args(["component", "install", "browser", "--json"])
        .env("A3S_BROWSER_EXECUTABLE", &executable)
        .env("A3S_USE_BROWSER_HOME", temp.path().join("managed"))
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["changed"], false);
    assert_eq!(value["data"]["provider"]["source"], "environment");
    assert_eq!(
        value["data"]["provider"]["path"],
        executable.to_string_lossy().as_ref()
    );
    assert!(!temp.path().join("managed/chrome").exists());
}

#[cfg(all(unix, feature = "browser"))]
#[test]
fn browser_install_command_delegates_to_the_external_driver() {
    let temp = tempfile::tempdir().unwrap();
    let driver = temp.path().join("browser-driver-fixture");
    let arguments = temp.path().join("arguments.txt");
    std::fs::write(
        &driver,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"schemaVersion\":1,\"ok\":true,\"data\":{{\"changed\":false}}}}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&driver).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&driver, permissions).unwrap();

    let output = Command::new(binary())
        .args(["browser", "install", "--json"])
        .env("A3S_USE_BROWSER_DRIVER", &driver)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(arguments).unwrap(),
        "install\n--json\n"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["changed"], false);
}

#[cfg(all(unix, feature = "browser"))]
#[test]
fn browser_upgrade_command_delegates_to_the_external_driver() {
    let temp = tempfile::tempdir().unwrap();
    let driver = temp.path().join("browser-driver-fixture");
    let arguments = temp.path().join("arguments.txt");
    std::fs::write(
        &driver,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"schemaVersion\":1,\"ok\":true,\"data\":{{\"changed\":true}}}}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&driver).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&driver, permissions).unwrap();

    let output = Command::new(binary())
        .args(["browser", "upgrade", "--json"])
        .env("A3S_USE_BROWSER_DRIVER", &driver)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(arguments).unwrap(),
        "upgrade\n--json\n"
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["changed"], true);
}

#[cfg(feature = "ocr")]
#[test]
fn built_in_ocr_projects_the_canonical_code_route_and_skill() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");

    let snapshot = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot: serde_json::Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    let ocr = snapshot["data"]["registry"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["id"] == "use/ocr")
        .unwrap();
    assert_eq!(ocr["route"], "ocr");
    assert_eq!(ocr["origin"], "built-in");
    assert_eq!(ocr["enabled"], true);
    assert_eq!(ocr["mcp"]["target"], "ocr-native");
    assert!(ocr["skills"][0]["path"]
        .as_str()
        .is_some_and(|path| Path::new(path).ends_with("skills/a3s-use-ocr/SKILL.md")));
    let digest = ocr["skills"][0]["sha256"].as_str().unwrap();
    assert_eq!(digest.len(), 64);
}
