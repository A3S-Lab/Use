#![cfg(all(unix, feature = "extensions"))]

use std::fs::File;
use std::os::unix::fs::PermissionsExt;
use std::process::Command;

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
}

#[test]
fn archived_extension_installs_dispatches_and_uninstalls_through_the_cli() {
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
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();

    let archive = temp.path().join("acme-slack.tar.gz");
    let file = File::create(&archive).unwrap();
    let encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);
    builder.append_dir_all("package", &package).unwrap();
    builder.finish().unwrap();
    drop(builder);

    let home = temp.path().join("home");
    let installed = Command::new(binary())
        .args([
            "component",
            "install",
            "acme/slack",
            "--from",
            archive.to_str().unwrap(),
            "--allow-unsigned",
            "--json",
        ])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(
        installed.status.success(),
        "status: {}\nstdout: {}\nstderr: {}",
        installed.status,
        String::from_utf8_lossy(&installed.stdout),
        String::from_utf8_lossy(&installed.stderr)
    );
    let installed_json: serde_json::Value = serde_json::from_slice(&installed.stdout).unwrap();
    assert_eq!(installed_json["data"]["component"]["id"], "acme/slack");

    let delegated = Command::new(binary())
        .args(["slack", "channels", "list", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert_eq!(delegated.status.code(), Some(7));
    assert_eq!(
        String::from_utf8(delegated.stdout).unwrap(),
        "acme/slack\nchannels list --json\n"
    );

    let removed = Command::new(binary())
        .args(["component", "uninstall", "acme/slack", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
    let removed_json: serde_json::Value = serde_json::from_slice(&removed.stdout).unwrap();
    assert_eq!(removed_json["data"]["changed"], true);

    let listed = Command::new(binary())
        .args(["extension", "list", "--json"])
        .env("A3S_USE_HOME", &home)
        .output()
        .unwrap();
    assert!(listed.status.success(), "{listed:?}");
    let listed_json: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(listed_json["data"]["extensions"], serde_json::json!([]));
    assert!(!home.join("data/extensions/acme/slack").exists());
    assert!(!home.join("state/extensions/acme/slack.json").exists());
}
