#[cfg(feature = "office")]
use std::io::Write;
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
    assert_eq!(value["data"]["domains"][0]["id"], "browser");
    assert_eq!(value["data"]["domains"][1]["id"], "office");
    assert_eq!(value["data"]["domains"][2]["id"], "box");
    assert!(value["data"].get("customJsonRpc").is_none());
    assert!(value.get("jsonrpc").is_none());
}

#[test]
fn unified_capability_snapshot_projects_builtin_skills() {
    let temp = tempfile::tempdir().unwrap();
    let office_skills = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates")
        .join("office")
        .join("skills");
    let output = Command::new(binary())
        .args(["capability", "snapshot", "--json"])
        .env("A3S_USE_HOME", temp.path())
        .env("A3S_USE_OFFICE_SKILLS_DIR", &office_skills)
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
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
    let office = capabilities
        .iter()
        .find(|capability| capability["id"] == "use/office")
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
    assert_eq!(office["origin"], "built-in");
    #[cfg(feature = "office")]
    {
        assert!(office["surfaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|surface| surface == "skill"));
        assert!(office["skills"][0]["path"].as_str().is_some_and(|path| {
            Path::new(path).ends_with(Path::new("skills").join("a3s-use-office").join("SKILL.md"))
        }));
        let office_skill_digest = office["skills"][0]["sha256"]
            .as_str()
            .expect("the Office capability must bind packaged Skill content");
        assert_eq!(office_skill_digest.len(), 64);
        assert!(office_skill_digest
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase()));
    }
    #[cfg(not(feature = "office"))]
    {
        assert_eq!(office["enabled"], false);
        assert_eq!(office["surfaces"], serde_json::json!([]));
        assert!(office.get("skills").is_none());
    }
    assert!(value.get("jsonrpc").is_none());
}

#[cfg(feature = "office")]
#[test]
fn office_skill_commands_are_packaged_and_provider_independent() {
    let temp = tempfile::tempdir().unwrap();
    let skills_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("crates")
        .join("office")
        .join("skills");
    let provider = temp.path().join("must-not-be-invoked");

    let list = Command::new(binary())
        .args(["office", "skills", "list", "--json"])
        .env("A3S_USE_OFFICE_SKILLS_DIR", &skills_root)
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(list.status.success(), "{list:?}");
    let list: serde_json::Value = serde_json::from_slice(&list.stdout).unwrap();
    assert_eq!(list["ok"], true);
    assert_eq!(list["data"][0]["name"], "a3s-use-office");

    let get = Command::new(binary())
        .args([
            "office",
            "skills",
            "get",
            "a3s-use-office",
            "--full",
            "--json",
        ])
        .env("A3S_USE_OFFICE_SKILLS_DIR", &skills_root)
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(get.status.success(), "{get:?}");
    let get: serde_json::Value = serde_json::from_slice(&get.stdout).unwrap();
    assert_eq!(get["data"]["name"], "a3s-use-office");
    assert_eq!(get["data"]["full"], true);
    assert!(get["data"]["content"]
        .as_str()
        .unwrap()
        .contains("## Bundled reference: references/mcp.md"));

    let path = Command::new(binary())
        .args(["office", "skills", "path", "a3s-use-office", "--json"])
        .env("A3S_USE_OFFICE_SKILLS_DIR", &skills_root)
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(path.status.success(), "{path:?}");
    let path: serde_json::Value = serde_json::from_slice(&path.stdout).unwrap();
    assert!(path["data"]["path"].as_str().is_some_and(|path| {
        Path::new(path).ends_with(Path::new("skills").join("a3s-use-office"))
    }));
    assert!(!provider.exists());
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

#[cfg(all(unix, feature = "office"))]
#[test]
fn office_route_preserves_native_cli_arguments_output_and_status() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("officecli-fixture");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$*\"\nexit 7\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let output = Command::new(binary())
        .args(["office", "get", "report.docx", "/body", "--json"])
        .env("A3S_OFFICECLI_EXECUTABLE", &executable)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "get report.docx /body --json\n"
    );
    assert!(output.stderr.is_empty());
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_reads_ooxml_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("native.docx");
    write_native_word_fixture(&document);

    let viewed = Command::new(binary())
        .args([
            "office",
            "native",
            "view",
            document.to_str().unwrap(),
            "text",
            "--json",
        ])
        .env_remove("A3S_OFFICECLI_EXECUTABLE")
        .output()
        .unwrap();

    assert!(viewed.status.success(), "{viewed:?}");
    let viewed: serde_json::Value = serde_json::from_slice(&viewed.stdout).unwrap();
    assert_eq!(viewed["data"]["view"], "text");
    assert_eq!(viewed["data"]["result"]["text"], "Native read");

    let queried = Command::new(binary())
        .args([
            "office",
            "native",
            "query",
            document.to_str().unwrap(),
            "p[style=Heading1]",
            "--json",
        ])
        .env_remove("A3S_OFFICECLI_EXECUTABLE")
        .output()
        .unwrap();
    assert!(queried.status.success(), "{queried:?}");
    let queried: serde_json::Value = serde_json::from_slice(&queried.stdout).unwrap();
    assert_eq!(queried["data"]["matches"], 1);
    assert_eq!(queried["data"]["results"][0]["path"], "/body/p[1]");
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_creates_all_ooxml_formats_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    for (extension, kind) in [
        ("docx", "word"),
        ("xlsx", "spreadsheet"),
        ("pptx", "presentation"),
    ] {
        let document = temp.path().join(format!("blank.{extension}"));
        let created = Command::new(binary())
            .args([
                "office",
                "native",
                "create",
                document.to_str().unwrap(),
                "--json",
            ])
            .env(
                "A3S_OFFICECLI_EXECUTABLE",
                temp.path().join("must-not-be-invoked"),
            )
            .output()
            .unwrap();

        assert!(created.status.success(), "{created:?}");
        let created: serde_json::Value = serde_json::from_slice(&created.stdout).unwrap();
        assert_eq!(created["data"]["operation"], "create");
        assert_eq!(created["data"]["kind"], kind);
        assert_eq!(created["data"]["created"], true);
        assert!(document.is_file());

        let validated = Command::new(binary())
            .args([
                "office",
                "native",
                "validate",
                document.to_str().unwrap(),
                "--json",
            ])
            .env(
                "A3S_OFFICECLI_EXECUTABLE",
                temp.path().join("must-not-be-invoked"),
            )
            .output()
            .unwrap();
        assert!(validated.status.success(), "{validated:?}");

        if extension == "xlsx" {
            let populated = Command::new(binary())
                .args([
                    "office",
                    "native",
                    "set",
                    document.to_str().unwrap(),
                    "/Sheet1/B2",
                    "--text",
                    "Created workbook cell",
                    "--json",
                ])
                .env(
                    "A3S_OFFICECLI_EXECUTABLE",
                    temp.path().join("must-not-be-invoked"),
                )
                .output()
                .unwrap();
            assert!(populated.status.success(), "{populated:?}");
            let populated: serde_json::Value = serde_json::from_slice(&populated.stdout).unwrap();
            assert_eq!(populated["data"]["node"]["path"], "/Sheet1/B2");
            assert_eq!(populated["data"]["node"]["text"], "Created workbook cell");
        }
    }
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_adds_and_populates_a_worksheet_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("native.xlsx");
    let created = Command::new(binary())
        .args([
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(created.status.success(), "{created:?}");

    let added = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/",
            "--type",
            "sheet",
            "--name",
            "Data",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(added.status.success(), "{added:?}");
    let added: serde_json::Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added["data"]["operation"], "add-worksheet");
    assert_eq!(added["data"]["node"]["path"], "/Data");

    let populated = Command::new(binary())
        .args([
            "office",
            "native",
            "set",
            document.to_str().unwrap(),
            "/Data/C3",
            "--text",
            "Native sheet",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(populated.status.success(), "{populated:?}");
    let populated: serde_json::Value = serde_json::from_slice(&populated.stdout).unwrap();
    assert_eq!(populated["data"]["node"]["text"], "Native sheet");

    let removed = Command::new(binary())
        .args([
            "office",
            "native",
            "remove",
            document.to_str().unwrap(),
            "/Data",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
    let removed: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["data"]["operation"], "remove");
    assert_eq!(removed["data"]["path"], "/Data");

    let root = Command::new(binary())
        .args([
            "office",
            "native",
            "get",
            document.to_str().unwrap(),
            "/",
            "--depth",
            "1",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(root.status.success(), "{root:?}");
    let root: serde_json::Value = serde_json::from_slice(&root.stdout).unwrap();
    assert_eq!(
        root["data"]["node"]["children"].as_array().unwrap().len(),
        1
    );
    assert_eq!(root["data"]["node"]["children"][0]["path"], "/Sheet1");

    let last_sheet = Command::new(binary())
        .args([
            "office",
            "native",
            "remove",
            document.to_str().unwrap(),
            "/Sheet1",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(!last_sheet.status.success(), "{last_sheet:?}");
    let last_sheet: serde_json::Value = serde_json::from_slice(&last_sheet.stdout).unwrap();
    assert_eq!(
        last_sheet["error"]["code"],
        "use.office.spreadsheet_last_sheet"
    );
    assert_eq!(native_office_text_view(&document, temp.path()), "");
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_writes_typed_spreadsheet_values_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("typed.xlsx");
    let mutations = temp.path().join("typed-mutations.json");
    let provider = temp.path().join("must-not-be-invoked");

    let created = Command::new(binary())
        .args([
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(created.status.success(), "{created:?}");

    for (reference, option, value, expected_text, expected_type) in [
        ("/Sheet1/A1", "--number", "42.5", "42.5", "Number"),
        ("/Sheet1/B1", "--boolean", "false", "false", "Boolean"),
    ] {
        let set = Command::new(binary())
            .args([
                "office",
                "native",
                "set",
                document.to_str().unwrap(),
                reference,
                option,
                value,
                "--json",
            ])
            .env("A3S_OFFICECLI_EXECUTABLE", &provider)
            .output()
            .unwrap();
        assert!(set.status.success(), "{set:?}");
        let set: serde_json::Value = serde_json::from_slice(&set.stdout).unwrap();
        assert_eq!(set["data"]["operation"], "set-cell-value");
        assert_eq!(set["data"]["node"]["text"], expected_text);
        assert_eq!(set["data"]["node"]["format"]["valueType"], expected_type);
    }

    let formula = Command::new(binary())
        .args([
            "office",
            "native",
            "set",
            document.to_str().unwrap(),
            "/Sheet1/C1",
            "--formula",
            "=A1*2",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(formula.status.success(), "{formula:?}");
    let formula: serde_json::Value = serde_json::from_slice(&formula.stdout).unwrap();
    assert_eq!(formula["data"]["node"]["format"]["formula"], "A1*2");

    std::fs::write(
        &mutations,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "mutations": [
                {
                    "operation": "set-cell-value",
                    "path": "/Sheet1/D1",
                    "value": {
                        "type": "formula",
                        "expression": "SUM(A1:C1)"
                    }
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let batched = Command::new(binary())
        .args([
            "office",
            "native",
            "batch",
            document.to_str().unwrap(),
            "--input",
            mutations.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(batched.status.success(), "{batched:?}");

    let invalid = Command::new(binary())
        .args([
            "office",
            "native",
            "set",
            document.to_str().unwrap(),
            "/Sheet1/E1",
            "--text",
            "ambiguous",
            "--number",
            "1",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(!invalid.status.success(), "{invalid:?}");
    let invalid: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(invalid["error"]["code"], "use.cli.invalid_usage");

    let formula = Command::new(binary())
        .args([
            "office",
            "native",
            "get",
            document.to_str().unwrap(),
            "/Sheet1/D1",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(formula.status.success(), "{formula:?}");
    let formula: serde_json::Value = serde_json::from_slice(&formula.stdout).unwrap();
    assert_eq!(formula["data"]["node"]["format"]["formula"], "SUM(A1:C1)");
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_mutates_and_saves_as_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("native.docx");
    let output = temp.path().join("updated.docx");
    write_native_word_fixture(&source);

    let updated = Command::new(binary())
        .args([
            "office",
            "native",
            "set",
            source.to_str().unwrap(),
            "/body/p[1]/r[1]",
            "--text",
            "Native write & preserve",
            "--output",
            output.to_str().unwrap(),
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();

    assert!(updated.status.success(), "{updated:?}");
    let updated: serde_json::Value = serde_json::from_slice(&updated.stdout).unwrap();
    assert_eq!(updated["data"]["operation"], "set-text");
    assert_eq!(updated["data"]["changed"], true);
    assert_eq!(updated["data"]["inPlace"], false);
    assert_eq!(updated["data"]["path"], "/body/p[1]/r[1]");

    let source_view = native_office_text_view(&source, temp.path());
    let output_view = native_office_text_view(&output, temp.path());
    assert_eq!(source_view, "Native read");
    assert_eq!(output_view, "Native write & preserve");
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_adds_a_word_paragraph_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("native.docx");
    let created = Command::new(binary())
        .args([
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(created.status.success(), "{created:?}");

    let added = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/body",
            "--type",
            "paragraph",
            "--text",
            "Added natively",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();

    assert!(added.status.success(), "{added:?}");
    let added: serde_json::Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added["data"]["operation"], "add-paragraph");
    assert_eq!(added["data"]["node"]["path"], "/body/p[2]");
    assert_eq!(added["data"]["node"]["text"], "Added natively");
    assert_eq!(
        native_office_text_view(&document, temp.path()),
        "Added natively"
    );

    let removed = Command::new(binary())
        .args([
            "office",
            "native",
            "remove",
            document.to_str().unwrap(),
            "/body/p[2]",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
    let removed: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["data"]["operation"], "remove");
    assert_eq!(removed["data"]["path"], "/body/p[2]");
    assert_eq!(native_office_text_view(&document, temp.path()), "");
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_structurally_edits_word_tables_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("tables.docx");
    let mutations = temp.path().join("table-mutations.json");
    let provider = temp.path().join("must-not-be-invoked");

    let created = Command::new(binary())
        .args([
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(created.status.success(), "{created:?}");

    std::fs::write(
        &mutations,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "mutations": [
                {
                    "operation": "add-table",
                    "parent": "/body",
                    "rows": 2,
                    "columns": 2
                },
                {
                    "operation": "set-text",
                    "path": "/body/tbl[1]/tr[1]/tc[1]",
                    "text": "Name"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();
    let batched = Command::new(binary())
        .args([
            "office",
            "native",
            "batch",
            document.to_str().unwrap(),
            "--input",
            mutations.to_str().unwrap(),
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(batched.status.success(), "{batched:?}");
    let batched: serde_json::Value = serde_json::from_slice(&batched.stdout).unwrap();
    assert_eq!(batched["data"]["result"]["applied"], 2);
    assert_eq!(batched["data"]["result"]["paths"][0], "/body/tbl[1]");

    let row = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/body/tbl[1]",
            "--type",
            "row",
            "--columns",
            "2",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(row.status.success(), "{row:?}");
    let row: serde_json::Value = serde_json::from_slice(&row.stdout).unwrap();
    assert_eq!(row["data"]["operation"], "add-table-row");
    assert_eq!(row["data"]["path"], "/body/tbl[1]/tr[3]");

    let cell = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/body/tbl[1]/tr[3]",
            "--type",
            "cell",
            "--text",
            "Extra",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(cell.status.success(), "{cell:?}");
    let cell: serde_json::Value = serde_json::from_slice(&cell.stdout).unwrap();
    assert_eq!(cell["data"]["operation"], "add-table-cell");
    assert_eq!(cell["data"]["path"], "/body/tbl[1]/tr[3]/tc[3]");
    assert_eq!(cell["data"]["node"]["text"], "Extra");

    let invalid = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/body",
            "--type",
            "table",
            "--rows",
            "0",
            "--columns",
            "2",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(!invalid.status.success(), "{invalid:?}");
    let invalid: serde_json::Value = serde_json::from_slice(&invalid.stdout).unwrap();
    assert_eq!(
        invalid["error"]["code"],
        "use.office.word_table_dimensions_invalid"
    );

    let removed_cell = Command::new(binary())
        .args([
            "office",
            "native",
            "remove",
            document.to_str().unwrap(),
            "/body/tbl[1]/tr[3]/tc[3]",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(removed_cell.status.success(), "{removed_cell:?}");
    let removed_row = Command::new(binary())
        .args([
            "office",
            "native",
            "remove",
            document.to_str().unwrap(),
            "/body/tbl[1]/tr[3]",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(removed_row.status.success(), "{removed_row:?}");

    let table = Command::new(binary())
        .args([
            "office",
            "native",
            "get",
            document.to_str().unwrap(),
            "/body/tbl[1]",
            "--depth",
            "3",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", &provider)
        .output()
        .unwrap();
    assert!(table.status.success(), "{table:?}");
    let table: serde_json::Value = serde_json::from_slice(&table.stdout).unwrap();
    assert_eq!(
        table["data"]["node"]["children"].as_array().unwrap().len(),
        2
    );
    assert_eq!(
        table["data"]["node"]["children"][0]["children"][0]["text"],
        "Name"
    );
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_adds_a_presentation_slide_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("native.pptx");
    let created = Command::new(binary())
        .args([
            "office",
            "native",
            "create",
            document.to_str().unwrap(),
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(created.status.success(), "{created:?}");

    let added = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/",
            "--type",
            "slide",
            "--text",
            "Native slide",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();

    assert!(added.status.success(), "{added:?}");
    let added: serde_json::Value = serde_json::from_slice(&added.stdout).unwrap();
    assert_eq!(added["data"]["operation"], "add-slide");
    assert_eq!(added["data"]["node"]["path"], "/slide[1]");
    assert_eq!(added["data"]["node"]["text"], "Native slide");
    assert_eq!(
        native_office_text_view(&document, temp.path()),
        "Native slide"
    );

    let shape = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/slide[1]",
            "--type",
            "shape",
            "--text",
            "Native body",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(shape.status.success(), "{shape:?}");
    let shape: serde_json::Value = serde_json::from_slice(&shape.stdout).unwrap();
    assert_eq!(shape["data"]["operation"], "add-shape");
    assert_eq!(shape["data"]["node"]["path"], "/slide[1]/shape[2]");
    assert_eq!(shape["data"]["node"]["text"], "Native body");

    let retained = Command::new(binary())
        .args([
            "office",
            "native",
            "add",
            document.to_str().unwrap(),
            "/",
            "--type",
            "slide",
            "--text",
            "Retained slide",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(retained.status.success(), "{retained:?}");

    let removed = Command::new(binary())
        .args([
            "office",
            "native",
            "remove",
            document.to_str().unwrap(),
            "/slide[1]",
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
    let removed: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed["data"]["operation"], "remove");
    assert_eq!(removed["data"]["path"], "/slide[1]");
    assert_eq!(
        native_office_text_view(&document, temp.path()),
        "Retained slide"
    );
}

#[cfg(feature = "office")]
#[test]
fn native_office_cli_batch_is_atomic_without_an_officecli_provider() {
    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("native.docx");
    let mutations = temp.path().join("mutations.json");
    write_native_word_fixture(&document);
    std::fs::write(
        &mutations,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "mutations": [
                {
                    "operation": "set-text",
                    "path": "/body/p[1]/r[1]",
                    "text": "must roll back"
                },
                {
                    "operation": "set-text",
                    "path": "/body/p[999]",
                    "text": "missing"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let failed = Command::new(binary())
        .args([
            "office",
            "native",
            "batch",
            document.to_str().unwrap(),
            "--input",
            mutations.to_str().unwrap(),
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();

    assert!(!failed.status.success(), "{failed:?}");
    let failed: serde_json::Value = serde_json::from_slice(&failed.stdout).unwrap();
    assert_eq!(failed["error"]["code"], "use.office.node_not_found");
    assert_eq!(
        native_office_text_view(&document, temp.path()),
        "Native read"
    );

    std::fs::write(
        &mutations,
        serde_json::to_vec(&serde_json::json!({
            "schemaVersion": 1,
            "mutations": [
                {
                    "operation": "set-text",
                    "path": "/body/p[1]/r[1]",
                    "text": "first"
                },
                {
                    "operation": "set-text",
                    "path": "/body/p[1]/r[1]",
                    "text": "committed"
                }
            ]
        }))
        .unwrap(),
    )
    .unwrap();

    let committed = Command::new(binary())
        .args([
            "office",
            "native",
            "batch",
            document.to_str().unwrap(),
            "--input",
            mutations.to_str().unwrap(),
            "--json",
        ])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .output()
        .unwrap();

    assert!(committed.status.success(), "{committed:?}");
    let committed: serde_json::Value = serde_json::from_slice(&committed.stdout).unwrap();
    assert_eq!(committed["data"]["result"]["applied"], 2);
    assert_eq!(committed["data"]["inPlace"], true);
    assert_eq!(native_office_text_view(&document, temp.path()), "committed");
}

#[cfg(feature = "office")]
fn native_office_text_view(path: &Path, temp: &Path) -> String {
    let output = Command::new(binary())
        .args([
            "office",
            "native",
            "view",
            path.to_str().unwrap(),
            "text",
            "--json",
        ])
        .env("A3S_OFFICECLI_EXECUTABLE", temp.join("must-not-be-invoked"))
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    output["data"]["result"]["text"]
        .as_str()
        .unwrap()
        .to_string()
}

#[cfg(feature = "office")]
fn write_native_word_fixture(path: &Path) {
    let file = std::fs::File::create(path).unwrap();
    let mut writer = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, bytes) in [
        (
            "[Content_Types].xml",
            br#"<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#
                .as_slice(),
        ),
        (
            "_rels/.rels",
            br#"<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#
                .as_slice(),
        ),
        (
            "word/document.xml",
            br#"<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:pPr><w:pStyle w:val="Heading1"/></w:pPr><w:r><w:t>Native read</w:t></w:r></w:p><w:sectPr/></w:body></w:document>"#
                .as_slice(),
        ),
    ] {
        writer.start_file(name, options).unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap();
}

#[cfg(all(unix, feature = "office"))]
#[test]
fn office_install_reuses_an_explicit_provider_without_downloading() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("officecli-fixture");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let output = Command::new(binary())
        .args(["component", "install", "office", "--json"])
        .env("A3S_OFFICECLI_EXECUTABLE", &executable)
        .env("A3S_USE_OFFICE_HOME", temp.path().join("managed"))
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
    assert!(!temp.path().join("managed/1.0.136").exists());
}

#[cfg(all(unix, feature = "office"))]
#[test]
fn office_mcp_target_delegates_to_officeclis_standard_server() {
    let temp = tempfile::tempdir().unwrap();
    let executable = temp.path().join("officecli-fixture");
    std::fs::write(&executable, "#!/bin/sh\nprintf '%s\\n' \"$*\"\nexit 5\n").unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();

    let output = Command::new(binary())
        .args(["mcp", "serve", "office"])
        .env("A3S_OFFICECLI_EXECUTABLE", &executable)
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(5));
    assert_eq!(String::from_utf8(output.stdout).unwrap(), "mcp\n");
    assert!(output.stderr.is_empty());
}

#[cfg(all(feature = "office", feature = "mcp"))]
#[tokio::test]
async fn native_office_mcp_is_standard_typed_and_independent_of_officecli() {
    use std::process::Stdio;
    use std::time::Duration;

    use tokio::io::{AsyncReadExt, AsyncWriteExt, BufReader};

    const RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

    let temp = tempfile::tempdir().unwrap();
    let document = temp.path().join("native-mcp.docx");
    let mut child = tokio::process::Command::new(binary())
        .args(["mcp", "serve", "office-native"])
        .env(
            "A3S_OFFICECLI_EXECUTABLE",
            temp.path().join("must-not-be-invoked"),
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut stderr = child.stderr.take().unwrap();

    let initialized = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "a3s-use-test", "version": "1" }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_eq!(initialized["jsonrpc"], "2.0");
    assert_eq!(initialized["id"], 1);
    assert_eq!(
        initialized["result"]["serverInfo"]["name"],
        "a3s-use-office-native"
    );

    stdin
        .write_all(
            b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/initialized\",\"params\":{}}\n",
        )
        .await
        .unwrap();
    stdin.flush().await.unwrap();

    let tools = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    let tools = tools["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 12);
    let apply = tools
        .iter()
        .find(|tool| tool["name"] == "office_apply_batch")
        .unwrap();
    assert_eq!(
        apply["inputSchema"]["properties"]["mutations"]["type"],
        "array"
    );
    assert!(apply["inputSchema"]["properties"].get("command").is_none());
    let apply_schema = apply["inputSchema"].to_string();
    for expected in ["underline", "script", "strikethrough", "superscript"] {
        assert!(apply_schema.contains(expected), "missing {expected}");
    }

    let created = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "office_create",
                "arguments": {
                    "session": "native_test",
                    "file": document
                }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_ne!(created["result"]["isError"], true);
    assert_eq!(
        created["result"]["structuredContent"]["session"],
        "native_test"
    );

    let applied = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "office_apply_batch",
                "arguments": {
                    "session": "native_test",
                    "mutations": [
                        {
                            "operation": "add-paragraph",
                            "parent": "/body",
                            "text": "Native MCP"
                        },
                        {
                            "operation": "set-text-format",
                            "path": "/body/p[2]/r[1]",
                            "format": {
                                "underline": "double",
                                "script": "superscript",
                                "strikethrough": true
                            }
                        }
                    ]
                }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_ne!(applied["result"]["isError"], true);
    assert_eq!(
        applied["result"]["structuredContent"]["result"]["applied"],
        2
    );
    assert_eq!(applied["result"]["structuredContent"]["persisted"], false);

    let formatted = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": {
                "name": "office_get",
                "arguments": {
                    "session": "native_test",
                    "path": "/body/p[2]/r[1]",
                    "depth": 0
                }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_ne!(formatted["result"]["isError"], true);
    let format = &formatted["result"]["structuredContent"]["node"]["format"];
    assert_eq!(format["underline"], "double");
    assert_eq!(format["script"], "superscript");
    assert_eq!(format["strike"], "true");

    let unsaved_close = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "office_close",
                "arguments": { "session": "native_test" }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_eq!(unsaved_close["result"]["isError"], true);
    assert_eq!(
        unsaved_close["result"]["structuredContent"]["code"],
        "use.office.unsaved_changes"
    );

    let saved = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "tools/call",
            "params": {
                "name": "office_save",
                "arguments": { "session": "native_test" }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_eq!(saved["result"]["structuredContent"]["saved"], true);

    let closed = standard_mcp_request(
        &mut stdin,
        &mut stdout,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 8,
            "method": "tools/call",
            "params": {
                "name": "office_close",
                "arguments": { "session": "native_test" }
            }
        }),
        RESPONSE_TIMEOUT,
    )
    .await;
    assert_eq!(closed["result"]["structuredContent"]["closed"], true);

    drop(stdin);
    let status = tokio::time::timeout(RESPONSE_TIMEOUT, child.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(status.success());
    let mut diagnostics = Vec::new();
    stderr.read_to_end(&mut diagnostics).await.unwrap();
    assert!(
        diagnostics.is_empty(),
        "{}",
        String::from_utf8_lossy(&diagnostics)
    );
    assert_eq!(
        native_office_text_view(&document, temp.path()),
        "Native MCP"
    );
}

#[cfg(all(feature = "office", feature = "mcp"))]
async fn standard_mcp_request(
    stdin: &mut tokio::process::ChildStdin,
    stdout: &mut tokio::io::BufReader<tokio::process::ChildStdout>,
    request: serde_json::Value,
    timeout: std::time::Duration,
) -> serde_json::Value {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let mut encoded = serde_json::to_vec(&request).unwrap();
    encoded.push(b'\n');
    stdin.write_all(&encoded).await.unwrap();
    stdin.flush().await.unwrap();
    let mut line = String::new();
    let bytes = tokio::time::timeout(timeout, stdout.read_line(&mut line))
        .await
        .unwrap()
        .unwrap();
    assert!(bytes > 0, "native Office MCP closed before responding");
    serde_json::from_str(&line).unwrap()
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
  schema_version = 1
  version = "1.0.0"
  route = "slack"
  actions = ["read"]

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
        .args(["component", "status", "acme/slack"])
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
    assert_eq!(unavailable_json["error"]["code"], "use.route_unknown");

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
        .args(["component", "uninstall", "acme/slack", "--json"])
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
        .args(["mcp", "serve", "acme/mcp-demo"])
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
