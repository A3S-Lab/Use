use std::process::Command;

#[cfg(all(unix, any(feature = "browser", feature = "extensions")))]
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
    assert!(value["data"].get("customJsonRpc").is_none());
    assert!(value.get("jsonrpc").is_none());
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

#[test]
fn mcp_stop_is_safe_when_no_service_is_running() {
    let output = Command::new(binary())
        .args(["mcp", "stop", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["data"]["protocol"], "mcp");
    assert_eq!(value["data"]["running"], false);
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

    let removed = Command::new(binary())
        .args(["component", "uninstall", "acme/slack", "--json"])
        .env("A3S_USE_HOME", temp.path().join("home"))
        .output()
        .unwrap();
    assert!(removed.status.success());
    let value: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(value["data"]["changed"], true);
}
