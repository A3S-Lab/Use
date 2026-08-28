use std::path::{Path, PathBuf};

use super::*;

#[test]
fn standalone_cli_installs_upgrades_observes_and_uninstalls_a3s_flow() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let first = cognitive_flow_target_version(&temp.path().join("first"), "1.0.0", &target);
    let next = cognitive_flow_target_version(&temp.path().join("next"), "1.1.0", &target);
    let repository = TestRepository::with_targets(vec![first, next], 71, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let compiler = fake_flow_compiler(temp.path());

    let installed =
        flow_registry_command(&server, &repository, &home, &compiler, "install", "1.0.0");
    assert!(installed.status.success(), "{installed:?}");
    assert_eq!(json(&installed)["data"]["changed"], true);
    let first_receipt = receipt(&home);
    let first_generation = first_receipt["lifecycleGeneration"].as_u64().unwrap();
    let first_package_root = PathBuf::from(first_receipt["packageRoot"].as_str().unwrap());
    assert!(first_package_root.is_dir());
    let first_bindings = flow_bindings(&home);
    assert_eq!(first_bindings.len(), 1, "{first_bindings:#?}");
    assert_eq!(first_bindings[0]["generation"], first_generation);
    assert_eq!(first_bindings[0]["surface"]["packageId"], "acme/flow-suite");
    assert_eq!(first_bindings[0]["surface"]["surface"]["id"], "reason");
    assert_flow_capability(&home, "1.0.0", first_generation);

    let upgraded =
        flow_registry_command(&server, &repository, &home, &compiler, "upgrade", "1.1.0");
    assert!(upgraded.status.success(), "{upgraded:?}");
    assert_eq!(json(&upgraded)["data"]["component"]["version"], "1.1.0");
    let next_receipt = receipt(&home);
    let next_generation = next_receipt["lifecycleGeneration"].as_u64().unwrap();
    let next_package_root = PathBuf::from(next_receipt["packageRoot"].as_str().unwrap());
    assert!(next_generation > first_generation);
    assert!(!first_package_root.exists());
    assert!(next_package_root.is_dir());
    let next_bindings = flow_bindings(&home);
    assert_eq!(next_bindings.len(), 1, "{next_bindings:#?}");
    assert_eq!(next_bindings[0]["generation"], next_generation);
    assert_ne!(
        next_bindings[0]["sourceDigest"],
        first_bindings[0]["sourceDigest"]
    );
    assert_flow_capability(&home, "1.1.0", next_generation);

    let mut removed = Command::new(binary());
    removed.args(["uninstall", "acme/flow-suite", "--json"]);
    append_test_installation_args(&mut removed);
    let removed = removed
        .env("A3S_USE_HOME", &home)
        .env("A3S_FLOW_NATIVE_TS_COMPILER", &compiler)
        .output()
        .unwrap();
    assert!(removed.status.success(), "{removed:?}");
    assert_eq!(json(&removed)["data"]["changed"], true);
    assert!(!receipt_path(&home).exists());
    assert!(!next_package_root.exists());
    assert!(flow_bindings(&home).is_empty());
    assert!(flow_capability(&home).is_none());
}

#[test]
fn missing_flow_compiler_fails_preflight_without_publishing_capabilities() {
    let temp = tempfile::tempdir().unwrap();
    let target = host_target();
    let package = cognitive_flow_target_version(temp.path(), "1.0.0", &target);
    let repository = TestRepository::with_targets(vec![package], 73, FUTURE);
    let server = TestServer::start(repository.routes.clone());
    let home = temp.path().join("home");
    let missing = flow_compiler_path(temp.path(), "missing-a3s-flow-native-compiler");

    let output = flow_registry_command(&server, &repository, &home, &missing, "install", "1.0.0");
    assert!(!output.status.success(), "{output:?}");
    assert_eq!(
        json(&output)["error"]["code"],
        "use.plugin.flow_preflight_failed"
    );
    let staged_receipt = receipt(&home);
    assert_eq!(staged_receipt["enabled"], false);
    let staged_generation = staged_receipt["lifecycleGeneration"].as_u64().unwrap();
    assert!(staged_generation > 0);
    assert!(flow_bindings(&home).is_empty());
    let staged_registry = capability_registry(&home);
    assert_eq!(staged_registry["generation"], 0);
    assert!(staged_registry["capabilities"]
        .as_array()
        .is_some_and(|capabilities| capabilities
            .iter()
            .all(|capability| capability["route"] != "flow-suite")));

    write_fake_flow_compiler(&missing);
    let retried = flow_registry_command(&server, &repository, &home, &missing, "install", "1.0.0");
    assert!(retried.status.success(), "{retried:?}");
    let published_receipt = receipt(&home);
    assert_eq!(published_receipt["enabled"], true);
    assert_eq!(published_receipt["lifecycleGeneration"], staged_generation);
    assert_eq!(flow_bindings(&home).len(), 1);
    assert_eq!(capability_registry(&home)["generation"], 1);
}

fn fake_flow_compiler(root: &Path) -> PathBuf {
    let compiler = flow_compiler_path(root, "a3s-flow-native-compiler");
    write_fake_flow_compiler(&compiler);
    compiler
}

fn flow_compiler_path(root: &Path, stem: &str) -> PathBuf {
    if cfg!(windows) {
        root.join(format!("{stem}.cmd"))
    } else {
        root.join(stem)
    }
}

#[cfg(unix)]
fn write_fake_flow_compiler(compiler: &Path) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::write(
        compiler,
        r#"#!/bin/sh
set -eu
[ "$1" = "compile" ]
shift
output=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-o" ]; then
    shift
    output="$1"
  fi
  shift
done
[ -n "$output" ]
printf '#!/bin/sh\nexit 0\n' > "$output"
chmod +x "$output"
"#,
    )
    .unwrap();
    let mut permissions = std::fs::metadata(compiler).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(compiler, permissions).unwrap();
}

#[cfg(windows)]
fn write_fake_flow_compiler(compiler: &Path) {
    std::fs::write(
        compiler,
        "@echo off\r\n\
setlocal EnableExtensions\r\n\
if /I not \"%~1\"==\"compile\" exit /b 2\r\n\
shift\r\n\
set \"output=\"\r\n\
:parse\r\n\
if \"%~1\"==\"\" goto done\r\n\
if /I \"%~1\"==\"-o\" goto output\r\n\
:next\r\n\
shift\r\n\
goto parse\r\n\
:output\r\n\
shift\r\n\
set \"output=%~1\"\r\n\
goto next\r\n\
:done\r\n\
if not defined output exit /b 3\r\n\
> \"%output%\" echo @echo off\r\n\
>> \"%output%\" echo exit /b 0\r\n\
exit /b 0\r\n",
    )
    .unwrap();
}

fn flow_registry_command(
    server: &TestServer,
    repository: &TestRepository,
    home: &Path,
    compiler: &Path,
    action: &str,
    version: &str,
) -> Output {
    configure_registry(server, repository, home, &[]);
    let mut command = Command::new(binary());
    command.args([
        action,
        "acme/flow-suite",
        "--registry-name",
        "fixture",
        "--version",
        version,
        "--json",
    ]);
    append_test_installation_args(&mut command);
    command
        .env("A3S_USE_HOME", home)
        .env("A3S_FLOW_NATIVE_TS_COMPILER", compiler)
        .output()
        .unwrap()
}

fn receipt_path(home: &Path) -> PathBuf {
    extension_paths(home)
        .state_root()
        .join("extensions/acme/flow-suite.json")
}

fn receipt(home: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(receipt_path(home)).unwrap()).unwrap()
}

fn flow_bindings(home: &Path) -> Vec<serde_json::Value> {
    fn collect(directory: &Path, output: &mut Vec<serde_json::Value>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(&path, output);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                output.push(serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap());
            }
        }
    }

    let mut bindings = Vec::new();
    collect(
        &extension_paths(home).state_root().join("bindings/flow"),
        &mut bindings,
    );
    bindings.sort_by_key(|binding| binding["generation"].as_u64().unwrap_or_default());
    bindings
}

fn flow_capability(home: &Path) -> Option<serde_json::Value> {
    capability_registry(home)["capabilities"]
        .as_array()
        .unwrap()
        .iter()
        .find(|capability| capability["route"] == "flow-suite")
        .cloned()
}

fn capability_registry(home: &Path) -> serde_json::Value {
    let mut snapshot = Command::new(binary());
    snapshot.args(["capability", "snapshot", "--json"]);
    append_test_installation_args(&mut snapshot);
    let snapshot = snapshot.env("A3S_USE_HOME", home).output().unwrap();
    assert!(snapshot.status.success(), "{snapshot:?}");
    json(&snapshot)["data"]["registry"].clone()
}

fn assert_flow_capability(home: &Path, version: &str, generation: u64) {
    let capability = flow_capability(home).expect("published Flow capability");
    assert_eq!(capability["version"], version);
    assert_eq!(capability["lifecycleGeneration"], generation);
    assert_eq!(capability["readiness"], "ready");
    assert_eq!(capability["flows"][0]["id"], "reason");
    assert_eq!(capability["flows"][0]["engine"], "a3s-flow");
    assert_eq!(capability["flows"][0]["runtime"], "native-ts");
    assert_eq!(capability["knowledge"][0]["generation"], generation);
    assert_eq!(capability["skills"].as_array().unwrap().len(), 1);
    assert_eq!(capability["activityBar"].as_array().unwrap().len(), 1);
}
