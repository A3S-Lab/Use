use std::path::Path;
use std::process::Command;
#[cfg(all(feature = "extensions", unix))]
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

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
fn browser_driver_session_listing_coexists_with_authenticated_standard_mcp() {
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

    let listed = Command::new(binary())
        .args(["browser", "session", "list", "--json"])
        .env("A3S_USE_RUNTIME_DIR", temp.path())
        .output()
        .unwrap();
    assert!(listed.status.success(), "{listed:?}");
    let listed_json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed_json["data"]["sessions"], serde_json::json!([]));

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

#[cfg(all(feature = "browser", feature = "mcp"))]
#[cfg_attr(
    windows,
    ignore = "Windows real-Chrome persistent sessions are roadmap; macOS and Linux are the current supported runtime platforms"
)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_session_state_survives_separate_cli_invocations_when_chrome_is_available() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const CLI_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(45);
    const FIXTURE_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);

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
#[tokio::test]
async fn browser_mcp_uses_the_standard_initialize_and_tools_contract() {
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

    const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

    let mut child = tokio::process::Command::new(binary())
        .args(["mcp", "serve", "browser", "--tools", "all"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());

    stdin
        .write_all(
            br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"a3s-use-test","version":"1"}}}
"#,
        )
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    let mut line = String::new();
    tokio::time::timeout(RESPONSE_TIMEOUT, stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    let initialized: serde_json::Value = serde_json::from_str(&line).unwrap();
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["id"], 1);
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "a3s-use-browser"
    );

    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
        )
        .await
        .unwrap();
    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"tools/list\",\"params\":{}}\n\
{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"tools/list\",\"params\":{\"cursor\":\"64\"}}\n\
{\"jsonrpc\":\"2.0\",\"id\":4,\"method\":\"tools/list\",\"params\":{\"cursor\":\"128\"}}\n",
        )
        .await
        .unwrap();
    stdin.flush().await.unwrap();
    let mut names = Vec::new();
    for _ in 0..3 {
        line.clear();
        tokio::time::timeout(RESPONSE_TIMEOUT, stdout.read_line(&mut line))
            .await
            .unwrap()
            .unwrap();
        let tools: serde_json::Value = serde_json::from_str(&line).unwrap();
        names.extend(
            tools["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(|tool| tool["name"].as_str().unwrap().to_string()),
        );
    }
    assert_eq!(names.len(), 151);
    assert!(names.iter().any(|name| name == "agent_browser_open"));
    assert!(names.iter().any(|name| name == "agent_browser_snapshot"));
    assert!(names
        .iter()
        .any(|name| name == "agent_browser_dashboard_start"));

    drop(stdin);
    let status = tokio::time::timeout(RESPONSE_TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
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
fn browser_install_command_delegates_to_the_component_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("chrome-fixture");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let output = Command::new(binary())
        .args(["browser", "install", "--json"])
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
fn browser_upgrade_delegates_only_to_the_a3s_component_lifecycle() {
    let temp = tempfile::tempdir().unwrap();
    let lifecycle = temp.path().join("a3s-use-fixture");
    let arguments = temp.path().join("arguments.txt");
    std::fs::write(
        &lifecycle,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nprintf '%s\\n' '{{\"schemaVersion\":1,\"ok\":true,\"data\":{{\"changed\":true}}}}'\n",
            arguments.display()
        ),
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&lifecycle).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&lifecycle, permissions).unwrap();

    let output = Command::new(binary())
        .args(["browser", "upgrade", "--json"])
        .env("A3S_USE_EXECUTABLE", &lifecycle)
        .output()
        .unwrap();

    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        std::fs::read_to_string(arguments).unwrap(),
        "component\ninstall\nbrowser\n--force\n--json\n"
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

#[cfg(all(unix, feature = "extensions"))]
#[test]
fn explicit_extension_install_delegates_native_cli_and_preserves_status() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("package");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("a3s-use-extension.acl"),
        r#"extension "acme/slack" {
  schema_version = 2
  version = "1.0.0"
  route = "slack"
  requires_use = ">=0.2.0, <0.3.0"
  actions = ["read"]

  repository {
    url = "https://github.com/acme/slack"
  }

  cli {
    executable = "bin/a3s-use-acme-slack"
    json_output = true
  }
}
"#,
    )
    .unwrap();
    let executable = package.join("bin/a3s-use-acme-slack");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf '%s\\n' \"$A3S_USE_EXTENSION_ID\"\nprintf '%s\\n' \"$*\"\nexit 7\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let installed = Command::new(binary())
        .args([
            "component",
            "install",
            "acme/slack",
            "--from",
            package.to_str().unwrap(),
            "--allow-unsigned",
            "--json",
        ])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(installed.status.success(), "{:?}", installed);
    let value: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(value["data"]["component"]["id"], "acme/slack");
    assert_eq!(value["data"]["component"]["surfaces"][0], "cli");

    let status_by_route = Command::new(binary())
        .args(["component", "status", "use/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(status_by_route.status.success(), "{status_by_route:?}");
    let status_by_route: serde_json::Value =
        serde_json::from_slice(&status_by_route.stdout).unwrap();
    assert_eq!(status_by_route["component"]["route"], "slack");
    assert_eq!(status_by_route["component"]["compatible"], true);
    assert_eq!(
        status_by_route["component"]["requiresUse"],
        ">=0.2.0, <0.3.0"
    );
    assert_eq!(
        status_by_route["component"]["repository"]["url"],
        "https://github.com/acme/slack"
    );

    let doctor_by_route = Command::new(binary())
        .args(["doctor", "slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(doctor_by_route.status.success(), "{doctor_by_route:?}");
    let doctor_by_route: serde_json::Value =
        serde_json::from_slice(&doctor_by_route.stdout).unwrap();
    assert_eq!(
        doctor_by_route["data"]["diagnostics"][0]["provider"],
        "acme/slack"
    );

    let snapshot = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot: serde_json::Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    let slack = snapshot["data"]["registry"]["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["route"] == "slack")
        .unwrap();
    assert_eq!(slack["requiresUse"], ">=0.2.0, <0.3.0");
    assert_eq!(slack["repository"]["url"], "https://github.com/acme/slack");

    let delegated = Command::new(binary())
        .args(["slack", "channels", "list", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert_eq!(delegated.status.code(), Some(7));
    assert_eq!(
        String::from_utf8(delegated.stdout).unwrap(),
        "acme/slack\nchannels list --json\n"
    );
    assert!(delegated.stderr.is_empty());

    for route_or_id in ["use/slack", "acme/slack", "use/acme/slack"] {
        let delegated = Command::new(binary())
            .args([route_or_id, "channels", "list", "--json"])
            .env("A3S_USE_HOME", temp.path().join("home"))
            .output()
            .unwrap();
        assert_eq!(
            delegated.status.code(),
            Some(7),
            "failed to delegate {route_or_id}: {delegated:?}"
        );
        assert_eq!(
            String::from_utf8(delegated.stdout).unwrap(),
            "acme/slack\nchannels list --json\n"
        );
        assert!(delegated.stderr.is_empty());
    }

    let disabled = Command::new(binary())
        .args(["extension", "disable", "acme/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(disabled.status.success(), "{disabled:?}");
    let disabled_json: serde_json::Value = serde_json::from_slice(&disabled.stdout).unwrap();
    assert_eq!(disabled_json["data"]["enabled"], false);
    assert_eq!(disabled_json["data"]["generation"], 2);

    let disabled_status = Command::new(binary())
        .args(["component", "status", "slack"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(disabled_status.status.success(), "{disabled_status:?}");
    assert!(String::from_utf8(disabled_status.stdout)
        .unwrap()
        .contains("is disabled on route 'slack'"));

    let unavailable = Command::new(binary())
        .args(["slack", "channels", "list", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(!unavailable.status.success());
    let unavailable_json: serde_json::Value = serde_json::from_slice(&unavailable.stdout).unwrap();
    assert_eq!(
        unavailable_json["error"]["code"],
        "use.extension.not_active"
    );

    let unavailable_alias = Command::new(binary())
        .args(["use/slack", "channels", "list", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(!unavailable_alias.status.success());
    let unavailable_alias_json: serde_json::Value =
        serde_json::from_slice(&unavailable_alias.stdout).unwrap();
    assert_eq!(
        unavailable_alias_json["error"]["code"],
        "use.extension.not_active"
    );

    let enabled = Command::new(binary())
        .args(["extension", "enable", "acme/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(enabled.status.success(), "{enabled:?}");
    let enabled_json: serde_json::Value = serde_json::from_slice(&enabled.stdout).unwrap();
    assert_eq!(enabled_json["data"]["enabled"], true);
    assert_eq!(enabled_json["data"]["generation"], 3);

    let snapshot = Command::new(binary())
        .args(["extension", "snapshot", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    let snapshot_json: serde_json::Value = serde_json::from_slice(&snapshot.stdout).unwrap();
    assert_eq!(snapshot_json["data"]["registry"]["generation"], 3);
    assert_eq!(
        snapshot_json["data"]["registry"]["routes"][0]["enabled"],
        true
    );

    let watcher = Command::new(binary())
        .args([
            "extension",
            "watch",
            "--after-generation",
            "3",
            "--timeout-ms",
            "2000",
            "--json",
        ])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let disabled_for_watch = Command::new(binary())
        .args(["extension", "disable", "acme/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(disabled_for_watch.status.success());
    let watched = watcher.wait_with_output().unwrap();
    assert!(watched.status.success(), "{watched:?}");
    let watched_json: serde_json::Value = serde_json::from_slice(&watched.stdout).unwrap();
    assert_eq!(watched_json["data"]["changed"], true);
    assert_eq!(watched_json["data"]["registry"]["generation"], 4);

    let reenabled = Command::new(binary())
        .args(["extension", "enable", "acme/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(reenabled.status.success());

    let removed = Command::new(binary())
        .args(["component", "uninstall", "use/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(removed.status.success());
    let value: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(value["data"]["changed"], true);
}

#[cfg(all(unix, feature = "extensions"))]
#[test]
fn disable_drains_a_real_delegated_process_before_returning() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("package");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("a3s-use-extension.acl"),
        r#"extension "acme/slow" {
  schema_version = 1
  version = "1.0.0"
  route = "slow"
  actions = ["read"]

  cli {
    executable = "bin/slow"
    json_output = false
  }
}
"#,
    )
    .unwrap();
    let executable = package.join("bin/slow");
    std::fs::write(
        &executable,
        "#!/bin/sh\nprintf started > \"$A3S_USE_PACKAGE_ROOT/started\"\nsleep 1\n",
    )
    .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    let home = temp.path().join("home");

    let installed = Command::new(binary())
        .args([
            "component",
            "install",
            "acme/slow",
            "--from",
            package.to_str().unwrap(),
            "--allow-unsigned",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(installed.status.success(), "{installed:?}");
    let receipt: serde_json::Value = serde_json::from_slice(
        &std::fs::read(home.join("state/extensions/acme/slow.json")).unwrap(),
    )
    .unwrap();
    let started =
        std::path::PathBuf::from(receipt["packageRoot"].as_str().unwrap()).join("started");

    let mut route = Command::new(binary())
        .arg("slow")
        .env("A3S_USE_HOME", &home)
        .spawn()
        .unwrap();
    for _ in 0..500 {
        if started.exists() {
            break;
        }
        if let Some(status) = route.try_wait().unwrap() {
            panic!("delegated process exited before its marker: {status}");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(started.exists(), "delegated process did not start");

    let before = Instant::now();
    let disabled = Command::new(binary())
        .args([
            "extension",
            "disable",
            "acme/slow",
            "--timeout-ms",
            "3000",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    let elapsed = before.elapsed();

    assert!(disabled.status.success(), "{disabled:?}");
    assert!(
        elapsed >= Duration::from_millis(600),
        "drained in {elapsed:?}"
    );
    assert!(route.wait().unwrap().success());
}

#[cfg(all(unix, feature = "extensions"))]
#[test]
fn external_mcp_target_launches_the_declared_standard_stdio_server() {
    let temp = tempfile::tempdir().unwrap();
    let package = temp.path().join("package");
    std::fs::create_dir_all(package.join("bin")).unwrap();
    std::fs::write(
        package.join("a3s-use-extension.acl"),
        r#"extension "acme/mcp-demo" {
  schema_version = 1
  version = "1.0.0"
  route = "mcp-demo"
  actions = ["read"]

  mcp {
    executable = "bin/server"
    args = ["--stdio", "fixture"]
    transport = "stdio"
  }
}
"#,
    )
    .unwrap();
    let executable = package.join("bin/server");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$*\"\nexit 6\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let installed = Command::new(binary())
        .args([
            "component",
            "install",
            "acme/mcp-demo",
            "--from",
            package.to_str().unwrap(),
            "--allow-unsigned",
            "--json",
        ])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(installed.status.success(), "{installed:?}");

    let delegated = Command::new(binary())
        .args(["mcp", "serve", "mcp-demo"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert_eq!(delegated.status.code(), Some(6));
    assert_eq!(
        String::from_utf8(delegated.stdout).unwrap(),
        "--stdio fixture\n"
    );
    assert!(delegated.stderr.is_empty());
}
