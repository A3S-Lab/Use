//! End-to-end tests for the registry tools binary: keygen → lint → pack →
//! assemble → verify, plus the failure paths that must stay closed.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

const MANIFEST: &str = "\
extension \"a3s/registry-demo\" {
  schema_version = 3
  version        = \"0.1.0\"
  route          = \"registry-demo\"
  requires_use   = \">=0.3.0, <0.4.0\"
  actions        = [\"read\"]

  repository {
    url      = \"https://github.com/A3S-Lab/Use-Registry\"
    revision = \"0123456789abcdef0123456789abcdef01234567\"
  }

  skill \"demo\" {
    path          = \"skills/demo/SKILL.md\"
    requires_tool = []
    requires_mcp  = []
    optional      = false
  }
}
";

const TOOL_MANIFEST: &str = "\
extension \"a3s/registry-echo\" {
  schema_version = 3
  version        = \"0.1.0\"
  requires_use   = \">=0.3.0, <0.4.0\"
  actions        = [\"read\", \"execute\"]

  repository {
    url      = \"https://github.com/A3S-Lab/Use-Registry\"
    revision = \"0123456789abcdef0123456789abcdef01234567\"
  }

  tool \"echo\" {
    workload    = \"task\"
    interface   = \"cli\"
    executable  = \"tools/echo\"
    command     = \"registry-echo\"
    json_output = true
    interactive = false
    timeout_ms  = 120000
    activation  = \"lazy\"
    optional    = false
  }
}
";

const PERMISSION_CEILING: &str = r#"{
  "schema": "a3s.use.plugin-permissions.v1",
  "surfaces": [
    {
      "surface": {"kind": "tool", "id": "echo"},
      "nativeExecution": true,
      "childProcess": false,
      "filesystem": [],
      "networkEgress": [],
      "privateService": false,
      "secrets": [],
      "resources": {
        "cpuMillis": 1000,
        "memoryBytes": 268435456,
        "pids": 64,
        "ephemeralStorageBytes": 1073741824,
        "taskTimeoutMs": 120000,
        "maxStdoutBytes": 1048576,
        "maxStderrBytes": 1048576
      },
      "uiHttp": []
    }
  ]
}"#;

fn binary() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a3s-use-registry-tools"))
}

fn run(command: &mut Command) -> (String, String, bool) {
    let output = command
        .output()
        .expect("the registry tools binary must run");
    (
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
        output.status.success(),
    )
}

fn write_skill_package(root: &Path) -> PathBuf {
    let package = root.join("packages/registry-demo");
    fs::create_dir_all(package.join("skills/demo")).unwrap();
    fs::write(package.join("a3s-use-extension.acl"), MANIFEST).unwrap();
    fs::write(
        package.join("README.md"),
        "# Registry demo\n\nSkill-only admission fixture.\n",
    )
    .unwrap();
    fs::write(
        package.join("skills/demo/SKILL.md"),
        "---\nname: registry-demo\ndescription: Registry admission demo skill\n---\n# Demo\n",
    )
    .unwrap();
    package
}

fn write_tool_package(root: &Path) -> PathBuf {
    let package = root.join("packages/registry-echo");
    fs::create_dir_all(package.join("tools")).unwrap();
    fs::write(package.join("a3s-use-extension.acl"), TOOL_MANIFEST).unwrap();
    fs::write(
        package.join("README.md"),
        "# Registry echo\n\nNative executable admission fixture.\n",
    )
    .unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::write(package.join("tools/echo"), "#!/bin/sh\nexec cat\n").unwrap();
        fs::set_permissions(
            package.join("tools/echo"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    #[cfg(not(unix))]
    fs::write(package.join("tools/echo"), "echo").unwrap();
    package
}

fn write_admissions(root: &Path, entries: &[(&str, &str, &str)]) -> PathBuf {
    let mut acl = String::new();
    for (package_id, directory, extra) in entries {
        acl.push_str(&format!(
            "admission \"{package_id}\" {{\n  package_directory = \"{directory}\"\n  channel = \"stable\"\n  target = \"any\"\n  display_name = \"{package_id} demo\"\n  description = \"Admission fixture for {package_id}.\"\n  license = \"Apache-2.0\"\n  keywords = [\"demo\"]\n  categories = [\"testing\"]\n{extra}}}\n\n"
        ));
    }
    let path = root.join("admissions.acl");
    fs::write(&path, acl).unwrap();
    path
}

fn setup_registry(root: &Path, entries: &[(&str, &str, &str)]) -> (PathBuf, String) {
    let keys = root.join("keys");
    let (_stdout, stderr, ok) =
        run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok, "keygen failed: {stderr}");
    assert!(keys.join("root.key").exists());

    let admissions = write_admissions(root, entries);
    let out_root = root.join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        out_root.to_str().unwrap(),
    ]));
    assert!(ok, "assemble failed: {stderr}");
    let outcome: Value = serde_json::from_str(&stdout).expect("assemble prints JSON");
    let root_sha256 = outcome["rootSha256"]
        .as_str()
        .expect("assemble reports the root digest")
        .to_owned();
    (out_root, root_sha256)
}

#[test]
fn skill_package_lints_before_pack() {
    let root = tempfile::tempdir().unwrap();
    let package = write_skill_package(root.path());
    let (stdout, stderr, ok) =
        run(binary().args(["lint", "--package-dir", package.to_str().unwrap()]));
    assert!(ok, "lint failed: {stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("lint prints JSON");
    assert_eq!(report["packageId"], "a3s/registry-demo");
    assert_eq!(report["version"], "0.1.0");
    assert_eq!(report["schemaVersion"], 3);
    assert!(report["manifestSha256"]
        .as_str()
        .unwrap()
        .starts_with("sha256:"));
    assert_eq!(report["fingerprint"]["sha256"].as_str().unwrap().len(), 64);
    assert!(report["surfaceKinds"]
        .as_array()
        .unwrap()
        .iter()
        .any(|kind| kind == "skill"));
}

#[test]
fn lint_fails_closed_when_skill_file_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let package = write_skill_package(root.path());
    fs::remove_file(package.join("skills/demo/SKILL.md")).unwrap();
    let (_stdout, stderr, ok) =
        run(binary().args(["lint", "--package-dir", package.to_str().unwrap()]));
    assert!(!ok, "lint must fail when a declared skill file is missing");
    assert!(
        stderr.contains("skill") || stderr.contains("Skill") || stderr.contains("README"),
        "unexpected lint error: {stderr}"
    );
}

#[test]
fn skill_package_assembles_and_verifies() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let (registry, root_sha256) = setup_registry(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );

    assert!(registry.join("metadata/root.json").exists());
    assert!(registry.join("metadata/targets.json").exists());
    assert!(registry.join("metadata/snapshot.json").exists());
    assert!(registry.join("metadata/timestamp.json").exists());
    let archive = registry.join(
        "targets/extensions/a3s/registry-demo/0.1.0/stable/any/a3s-registry-demo-0.1.0-any.tar.gz",
    );
    assert!(archive.exists(), "the archive target must be published");

    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        &root_sha256,
    ]));
    assert!(ok, "verify failed: {stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("verify prints JSON");
    assert_eq!(report["targetsChecked"], 1);
    assert!(report["packages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value
            .as_str()
            .unwrap()
            .starts_with("a3s/registry-demo@0.1.0")));
}

#[test]
fn tool_package_carries_a_planning_target() {
    let root = tempfile::tempdir().unwrap();
    write_tool_package(root.path());
    fs::write(root.path().join("ceiling.json"), PERMISSION_CEILING).unwrap();
    let (registry, _root_sha256) = setup_registry(
        root.path(),
        &[(
            "a3s/registry-echo",
            "packages/registry-echo",
            "  permission_ceiling = \"ceiling.json\"\n",
        )],
    );

    let planning =
        registry.join("targets/extensions/a3s/registry-echo/0.1.0/stable/any/planning-v1.json");
    assert!(
        planning.exists(),
        "executable packages need a planning target"
    );
    let bundle: Value = serde_json::from_str(&fs::read_to_string(&planning).unwrap()).unwrap();
    assert_eq!(bundle["schema"], "a3s.use.plugin-planning-bundle.v1");
    assert_eq!(bundle["packageId"], "a3s/registry-echo");
    assert_eq!(bundle["surfaces"][0]["kind"], "tool-task-native");

    let (stdout, stderr, ok) =
        run(binary().args(["verify", "--registry", registry.to_str().unwrap()]));
    assert!(ok, "verify failed: {stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("verify prints JSON");
    assert_eq!(report["targetsChecked"], 2);
}

#[test]
fn tampered_target_fails_verification() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let (registry, root_sha256) = setup_registry(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let archive = registry.join(
        "targets/extensions/a3s/registry-demo/0.1.0/stable/any/a3s-registry-demo-0.1.0-any.tar.gz",
    );
    let bytes = fs::read(&archive).unwrap();
    let mut tampered = bytes.clone();
    let last = tampered.len() - 1;
    tampered[last] ^= 0xff;
    fs::write(&archive, tampered).unwrap();

    let (_stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        &root_sha256,
    ]));
    assert!(!ok, "a tampered target must fail verification");
    assert!(stderr.contains("does not match its signed digest"));
}

#[test]
fn admission_identity_mismatch_fails_assembly() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/someone-else", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));

    let (_stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        root.path().join("registry").to_str().unwrap(),
    ]));
    assert!(
        !ok,
        "an admission must not adopt another package's identity"
    );
    assert!(stderr.contains("does not match the package manifest identity"));
}

#[test]
fn executable_package_without_ceiling_fails_assembly() {
    let root = tempfile::tempdir().unwrap();
    write_tool_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-echo", "packages/registry-echo", "")],
    );
    let keys = root.path().join("keys");
    run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));

    let (_stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        root.path().join("registry").to_str().unwrap(),
    ]));
    assert!(
        !ok,
        "executable surfaces require an explicit permission ceiling"
    );
    assert!(stderr.contains("must declare permission_ceiling"));
}

#[test]
fn keygen_refuses_to_overwrite_existing_keys() {
    let root = tempfile::tempdir().unwrap();
    let keys = root.path().join("keys");
    let (_stdout, _stderr, ok) =
        run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok);
    let (_stdout, stderr, ok) =
        run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(
        !ok,
        "a rerun must not silently rotate the registry identity"
    );
    assert!(stderr.contains("Refusing to overwrite"));
}

#[test]
fn republication_advances_the_metadata_version_the_client_accepts() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));

    let registry = root.path().join("registry");
    for version in [1_u64, 2, 3] {
        let (stdout, stderr, ok) = run(binary().args([
            "assemble",
            "--keys-dir",
            keys.to_str().unwrap(),
            "--admissions",
            admissions.to_str().unwrap(),
            "--out-root",
            registry.to_str().unwrap(),
            "--metadata-version",
            &version.to_string(),
        ]));
        assert!(ok, "assemble v{version} failed: {stderr}");
        let outcome: Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(
            outcome["metadataVersion"], version,
            "assembly reports the version"
        );
        let (_stdout, stderr, ok) =
            run(binary().args(["verify", "--registry", registry.to_str().unwrap()]));
        assert!(ok, "verify v{version} failed: {stderr}");
        let targets: Value = serde_json::from_str(
            &fs::read_to_string(registry.join("metadata/targets.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(targets["signed"]["version"], version);
    }
}

#[test]
fn offline_custody_recovery_rebuilds_the_same_bootstrap_pin() {
    // Exercises docs/registry-key-custody.md offline recovery:
    // restore keys, rebuild from the same admissions, verify the pin.
    // Threshold multi-custodian ceremony remains a separate production drill.
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok, "keygen failed: {stderr}\n{stdout}");

    let registry = root.path().join("registry");
    let expires = "2099-01-01T00:00:00Z";
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
        "--root-expires",
        expires,
        "--metadata-expires",
        expires,
    ]));
    assert!(ok, "initial assemble failed: {stderr}\n{stdout}");
    let first: Value = serde_json::from_str(&stdout).expect("assemble prints JSON");
    let pin = first["rootSha256"]
        .as_str()
        .expect("assemble reports the root digest")
        .to_string();

    fs::remove_dir_all(&registry).expect("wipe staged registry for recovery");
    assert!(!registry.exists());

    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
        "--root-expires",
        expires,
        "--metadata-expires",
        expires,
    ]));
    assert!(ok, "recovery assemble failed: {stderr}\n{stdout}");
    let recovered: Value = serde_json::from_str(&stdout).expect("recovery assemble prints JSON");
    assert_eq!(
        recovered["rootSha256"].as_str(),
        Some(pin.as_str()),
        "offline recovery must reproduce the bootstrap pin for identical inputs"
    );

    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        &pin,
    ]));
    assert!(ok, "recovery verify failed: {stderr}\n{stdout}");
    let report: Value = serde_json::from_str(&stdout).expect("verify prints JSON");
    assert_eq!(report["targetsChecked"], 1);
    assert!(report["packages"]
        .as_array()
        .unwrap()
        .iter()
        .any(|value| value
            .as_str()
            .unwrap()
            .starts_with("a3s/registry-demo@0.1.0")));
}

#[test]
fn threshold_root_ceremony_assembles_and_verifies_with_two_of_three_shares() {
    // Offline threshold drill from docs/registry-key-custody.md:
    // three root shares, threshold 2, assemble with all shares present.
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args([
        "keygen",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--root-share-count",
        "3",
        "--root-threshold",
        "2",
    ]));
    assert!(ok, "threshold keygen failed: {stderr}\n{stdout}");
    let keygen: Value = serde_json::from_str(&stdout).expect("keygen prints JSON");
    assert_eq!(keygen["rootPolicy"]["threshold"], 2);
    assert_eq!(keygen["rootPolicy"]["shareCount"], 3);
    assert_eq!(keygen["roleKeyIds"]["root"].as_array().unwrap().len(), 3);
    assert!(keys.join("root-0.key").exists());
    assert!(keys.join("root-1.key").exists());
    assert!(keys.join("root-2.key").exists());
    assert!(keys.join("root.policy.json").exists());
    assert!(!keys.join("root.key").exists());

    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
    ]));
    assert!(ok, "threshold assemble failed: {stderr}\n{stdout}");
    let assembled: Value = serde_json::from_str(&stdout).expect("assemble prints JSON");
    let pin = assembled["rootSha256"].as_str().unwrap().to_string();

    let root_meta: Value =
        serde_json::from_slice(&fs::read(registry.join("metadata").join("root.json")).unwrap())
            .unwrap();
    assert_eq!(root_meta["signed"]["roles"]["root"]["threshold"], 2);
    assert_eq!(
        root_meta["signed"]["roles"]["root"]["keyids"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(root_meta["signatures"].as_array().unwrap().len(), 3);

    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        &pin,
    ]));
    assert!(ok, "threshold verify failed: {stderr}\n{stdout}");
}

#[test]
fn threshold_assemble_fails_closed_when_too_few_root_shares_are_present() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args([
        "keygen",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--root-share-count",
        "3",
        "--root-threshold",
        "2",
    ]));
    assert!(ok, "threshold keygen failed: {stderr}\n{stdout}");
    fs::remove_file(keys.join("root-2.key")).unwrap();
    fs::remove_file(keys.join("root-1.key")).unwrap();

    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
    ]));
    assert!(!ok, "assemble must refuse under-threshold root custody");
    assert!(
        stderr.contains("registry_tools.keys_read_failed")
            || stdout.contains("registry_tools.keys_read_failed"),
        "stderr={stderr}\nstdout={stdout}"
    );
}

#[test]
fn root_rotation_retains_the_previous_root_and_requires_a_new_bootstrap_pin() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let previous_keys = root.path().join("keys-previous");
    let (stdout, stderr, ok) = run(binary().args([
        "keygen",
        "--keys-dir",
        previous_keys.to_str().unwrap(),
        "--root-share-count",
        "3",
        "--root-threshold",
        "2",
    ]));
    assert!(ok, "previous keygen failed: {stderr}\n{stdout}");

    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        previous_keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
    ]));
    assert!(ok, "assemble failed: {stderr}\n{stdout}");
    let assembled: Value = serde_json::from_str(&stdout).unwrap();
    let old_pin = assembled["rootSha256"].as_str().unwrap().to_string();

    let next_keys = root.path().join("keys-next");
    let (stdout, stderr, ok) = run(binary().args([
        "keygen",
        "--keys-dir",
        next_keys.to_str().unwrap(),
        "--root-share-count",
        "3",
        "--root-threshold",
        "2",
    ]));
    assert!(ok, "next keygen failed: {stderr}\n{stdout}");

    let (stdout, stderr, ok) = run(binary().args([
        "rotate-root",
        "--registry",
        registry.to_str().unwrap(),
        "--previous-keys-dir",
        previous_keys.to_str().unwrap(),
        "--next-keys-dir",
        next_keys.to_str().unwrap(),
    ]));
    assert!(ok, "rotate-root failed: {stderr}\n{stdout}");
    let rotated: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(rotated["previousRootVersion"], 1);
    assert_eq!(rotated["rootVersion"], 2);
    assert_ne!(rotated["rootSha256"].as_str().unwrap(), old_pin.as_str());
    assert!(registry.join("metadata/root.history/root.1.json").exists());

    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        old_pin.as_str(),
    ]));
    assert!(!ok, "old bootstrap pin must fail after rotation");
    assert!(
        stderr.contains("registry_tools.verify_failed")
            || stdout.contains("registry_tools.verify_failed"),
        "stderr={stderr}\nstdout={stdout}"
    );

    let new_pin = rotated["rootSha256"].as_str().unwrap();
    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        new_pin,
    ]));
    assert!(ok, "new bootstrap pin must verify: {stderr}\n{stdout}");
}

#[test]
fn root_rotation_fails_closed_when_previous_keys_do_not_match_published_root() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let published_keys = root.path().join("keys-published");
    let (stdout, stderr, ok) =
        run(binary().args(["keygen", "--keys-dir", published_keys.to_str().unwrap()]));
    assert!(ok, "published keygen failed: {stderr}\n{stdout}");
    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        published_keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
    ]));
    assert!(ok, "assemble failed: {stderr}\n{stdout}");

    let wrong_previous = root.path().join("keys-wrong");
    let next_keys = root.path().join("keys-next");
    let (stdout, stderr, ok) =
        run(binary().args(["keygen", "--keys-dir", wrong_previous.to_str().unwrap()]));
    assert!(ok, "wrong previous keygen failed: {stderr}\n{stdout}");
    let (stdout, stderr, ok) =
        run(binary().args(["keygen", "--keys-dir", next_keys.to_str().unwrap()]));
    assert!(ok, "next keygen failed: {stderr}\n{stdout}");

    let (stdout, stderr, ok) = run(binary().args([
        "rotate-root",
        "--registry",
        registry.to_str().unwrap(),
        "--previous-keys-dir",
        wrong_previous.to_str().unwrap(),
        "--next-keys-dir",
        next_keys.to_str().unwrap(),
    ]));
    assert!(!ok, "rotation must refuse mismatched previous custody");
    assert!(
        stderr.contains("registry_tools.rotate_failed")
            || stdout.contains("registry_tools.rotate_failed"),
        "stderr={stderr}\nstdout={stdout}"
    );
}

#[test]
fn republication_with_a_new_package_version_keeps_one_identity_per_target() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    let registry = root.path().join("registry");
    let assemble = |version: u64| {
        let (_stdout, stderr, ok) = run(binary().args([
            "assemble",
            "--keys-dir",
            keys.to_str().unwrap(),
            "--admissions",
            admissions.to_str().unwrap(),
            "--out-root",
            registry.to_str().unwrap(),
            "--metadata-version",
            &version.to_string(),
        ]));
        assert!(ok, "assemble failed: {stderr}");
    };
    assemble(1);

    // Publish 0.2.0 while the 0.1.0 tree stays on disk: the stale archive
    // is gone from the signed catalog, and the new identity verifies.
    let manifest_path = root
        .path()
        .join("packages/registry-demo/a3s-use-extension.acl");
    let manifest = fs::read_to_string(&manifest_path).unwrap();
    fs::write(&manifest_path, manifest.replace("0.1.0", "0.2.0")).unwrap();
    assemble(2);
    let (_stdout, stderr, ok) =
        run(binary().args(["verify", "--registry", registry.to_str().unwrap()]));
    assert!(ok, "verify after version bump failed: {stderr}");
    assert!(
        !registry
            .join("targets/extensions/a3s/registry-demo/0.1.0")
            .exists(),
        "the superseded version directory must not stay published",
    );
    assert!(registry
        .join("targets/extensions/a3s/registry-demo/0.2.0/stable/any/a3s-registry-demo-0.2.0-any.tar.gz")
        .exists());
}

#[test]
fn compose_package_wires_tool_mcp_okf_skill_and_ui() {
    let root = tempfile::tempdir().unwrap();
    let package = root.path().join("packages/mock-compose");
    fs::create_dir_all(package.join("okf/domain/concepts")).unwrap();
    fs::create_dir_all(package.join("skills/compose")).unwrap();
    fs::create_dir_all(package.join("ui/compose")).unwrap();
    fs::create_dir_all(package.join("tools")).unwrap();
    fs::create_dir_all(package.join("mcp")).unwrap();
    fs::write(
        package.join("a3s-use-extension.acl"),
        r#"
extension "a3s/mock-compose" {
  schema_version = 3
  version        = "0.1.0"
  route          = "mock-compose"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read", "execute"]

  repository {
    url      = "https://github.com/A3S-Lab/Use-Registry"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }

  tool "echo" {
    workload    = "task"
    interface   = "cli"
    executable  = "tools/echo"
    command     = "mock-compose-echo"
    json_output = true
    interactive = false
    timeout_ms  = 30000
    activation  = "lazy"
    optional    = false
  }

  mcp "context" {
    transport  = "stdio"
    executable = "mcp/context"
    args       = ["--stdio"]
    activation = "lazy"
    optional   = false
  }

  okf "domain" {
    format_version         = "0.2"
    root                   = "okf/domain"
    content_digest         = "sha256:355b6f00153630b082e60a0f7e0b67fbbb74b2a29067bca481f7eefecbb86c7a"
    concept_count          = 1
    file_count             = 2
    expanded_bytes         = 427
    max_files              = 64
    max_concepts           = 32
    max_expanded_bytes     = 1048576
    max_document_bytes     = 262144
    max_links_per_document = 128
    optional               = false
  }

  skill "compose" {
    path          = "skills/compose/SKILL.md"
    requires_tool = ["echo"]
    requires_mcp  = ["context"]
    requires_okf  = ["domain"]
    optional      = false
  }

  ui "compose" {
    entry     = "ui/compose/index.html"
    styles    = ["ui/compose/index.css"]
    scripts   = ["ui/compose/index.js"]
    skill     = "compose"
    bind_tool = ["echo"]
    bind_mcp  = ["context"]
    optional  = false
  }
}
"#,
    )
    .unwrap();
    fs::write(package.join("README.md"), "# compose\n").unwrap();
    fs::write(package.join("tools/echo"), "#!/bin/sh\nexec cat\n").unwrap();
    fs::write(package.join("mcp/context"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(
            package.join("tools/echo"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::set_permissions(
            package.join("mcp/context"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
    }
    fs::write(
        package.join("okf/domain/index.md"),
        "---\nokf_version: \"0.2\"\n---\n\n# Cognitive package knowledge\n\n- [Lifecycle](concepts/lifecycle.md) - One generation owns every plugin surface.\n",
    )
    .unwrap();
    fs::write(
        package.join("okf/domain/concepts/lifecycle.md"),
        "---\ntype: Project-Specific Decision\ntitle: Atomic cognitive package lifecycle\ndescription: Tool, MCP, OKF, Flow, Skill, and UI contributions activate as one package generation.\nstatus: stable\n---\n\n# Decision\n\nPublish package capabilities only after every required contribution is ready.\n",
    )
    .unwrap();
    fs::write(
        package.join("skills/compose/SKILL.md"),
        "---\nname: mock-compose\ndescription: compose\n---\n# Compose\n",
    )
    .unwrap();
    fs::write(package.join("ui/compose/index.html"), "<html></html>\n").unwrap();
    fs::write(package.join("ui/compose/index.css"), "body{}\n").unwrap();
    fs::write(package.join("ui/compose/index.js"), "console.log(1)\n").unwrap();
    fs::write(
        root.path().join("compose-ceiling.json"),
        r#"{
  "schema": "a3s.use.plugin-permissions.v1",
  "surfaces": [
    {
      "surface": {"kind": "mcp", "id": "context"},
      "nativeExecution": true,
      "childProcess": false,
      "filesystem": [],
      "networkEgress": [],
      "privateService": false,
      "secrets": [],
      "resources": {
        "cpuMillis": 500,
        "memoryBytes": 268435456,
        "pids": 32,
        "ephemeralStorageBytes": 536870912
      },
      "uiHttp": []
    },
    {
      "surface": {"kind": "tool", "id": "echo"},
      "nativeExecution": true,
      "childProcess": false,
      "filesystem": [],
      "networkEgress": [],
      "privateService": false,
      "secrets": [],
      "resources": {
        "cpuMillis": 1000,
        "memoryBytes": 268435456,
        "pids": 64,
        "ephemeralStorageBytes": 1073741824,
        "taskTimeoutMs": 30000,
        "maxStdoutBytes": 1048576,
        "maxStderrBytes": 1048576
      },
      "uiHttp": []
    }
  ]
}"#,
    )
    .unwrap();

    let (registry, root_sha256) = setup_registry(
        root.path(),
        &[(
            "a3s/mock-compose",
            "packages/mock-compose",
            "  permission_ceiling = \"compose-ceiling.json\"\n",
        )],
    );
    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        &root_sha256,
    ]));
    assert!(ok, "verify failed: {stderr}");
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["targetsChecked"], 2);

    let planning =
        registry.join("targets/extensions/a3s/mock-compose/0.1.0/stable/any/planning-v1.json");
    let bundle: Value = serde_json::from_str(&fs::read_to_string(planning).unwrap()).unwrap();
    let kinds: Vec<&str> = bundle["surfaces"]
        .as_array()
        .unwrap()
        .iter()
        .map(|surface| surface["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["mcp-stdio", "tool-task-native"]);
}

#[test]
fn check_expiry_passes_for_a_freshly_assembled_registry() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok, "keygen failed: {stderr}\n{stdout}");
    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
    ]));
    assert!(ok, "assemble failed: {stderr}\n{stdout}");
    let (stdout, stderr, ok) = run(binary().args([
        "check-expiry",
        "--registry",
        registry.to_str().unwrap(),
        "--warn-within-hours",
        "24",
    ]));
    assert!(
        ok,
        "fresh registry must pass expiry check: {stderr}\n{stdout}"
    );
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert!(report["expired"].as_array().unwrap().is_empty());
    assert!(report["expiringSoon"].as_array().unwrap().is_empty());
}

#[test]
fn check_expiry_fails_closed_when_metadata_expires_inside_the_warn_window() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok, "keygen failed: {stderr}\n{stdout}");
    // One hour ahead: inside the default 72h warn window.
    let soon = (std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + 3600) as i64;
    let expires = format_unix_rfc3339(soon);
    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
        "--root-expires",
        &expires,
        "--metadata-expires",
        &expires,
    ]));
    assert!(ok, "assemble failed: {stderr}\n{stdout}");
    let (stdout, stderr, ok) = run(binary().args([
        "check-expiry",
        "--registry",
        registry.to_str().unwrap(),
        "--warn-within-hours",
        "72",
    ]));
    assert!(!ok, "near-expiry registry must fail closed");
    assert!(
        stderr.contains("registry_tools.expiry_warning")
            || stdout.contains("registry_tools.expiry_warning"),
        "stderr={stderr}\nstdout={stdout}"
    );
}

fn format_unix_rfc3339(secs: i64) -> String {
    let days_since_epoch = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400) as u64;
    let z = days_since_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 }.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;
    let second = time_of_day % 60;
    format!("{y:04}-{m:02}-{d:02}T{hour:02}:{minute:02}:{second:02}Z")
}

#[test]
fn compare_mirrors_accepts_identical_trees_and_rejects_drift() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok, "keygen failed: {stderr}\n{stdout}");
    let left = root.path().join("left");
    let right = root.path().join("right");
    let expires = "2099-01-01T00:00:00Z";
    for out in [&left, &right] {
        let (stdout, stderr, ok) = run(binary().args([
            "assemble",
            "--keys-dir",
            keys.to_str().unwrap(),
            "--admissions",
            admissions.to_str().unwrap(),
            "--out-root",
            out.to_str().unwrap(),
            "--root-expires",
            expires,
            "--metadata-expires",
            expires,
        ]));
        assert!(ok, "assemble failed: {stderr}\n{stdout}");
    }
    let (stdout, stderr, ok) = run(binary().args([
        "compare-mirrors",
        "--left",
        left.to_str().unwrap(),
        "--right",
        right.to_str().unwrap(),
    ]));
    assert!(
        ok,
        "identical mirrors must compare equal: {stderr}\n{stdout}"
    );

    // Drift a target byte under the right tree.
    let target = find_first_file(&right.join("targets")).expect("assembled tree has targets");
    let mut bytes = std::fs::read(&target).unwrap();
    bytes.push(b'x');
    std::fs::write(&target, bytes).unwrap();
    let (stdout, stderr, ok) = run(binary().args([
        "compare-mirrors",
        "--left",
        left.to_str().unwrap(),
        "--right",
        right.to_str().unwrap(),
    ]));
    assert!(!ok, "drifted mirror must fail closed");
    assert!(
        stderr.contains("registry_tools.mirror_mismatch")
            || stdout.contains("registry_tools.mirror_mismatch"),
        "stderr={stderr}\nstdout={stdout}"
    );
}

fn find_first_file(root: &Path) -> Option<PathBuf> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(current) = stack.pop() {
        for entry in std::fs::read_dir(&current).ok()? {
            let entry = entry.ok()?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                return Some(path);
            }
        }
    }
    None
}

#[test]
fn withdraw_targets_removes_a_package_while_keeping_the_bootstrap_pin() {
    let root = tempfile::tempdir().unwrap();
    write_skill_package(root.path());
    let admissions = write_admissions(
        root.path(),
        &[("a3s/registry-demo", "packages/registry-demo", "")],
    );
    let keys = root.path().join("keys");
    let (stdout, stderr, ok) = run(binary().args(["keygen", "--keys-dir", keys.to_str().unwrap()]));
    assert!(ok, "keygen failed: {stderr}\n{stdout}");
    let expires = "2099-01-01T00:00:00Z";
    let registry = root.path().join("registry");
    let (stdout, stderr, ok) = run(binary().args([
        "assemble",
        "--keys-dir",
        keys.to_str().unwrap(),
        "--admissions",
        admissions.to_str().unwrap(),
        "--out-root",
        registry.to_str().unwrap(),
        "--root-expires",
        expires,
        "--metadata-expires",
        expires,
    ]));
    assert!(ok, "assemble failed: {stderr}\n{stdout}");
    let assembled: Value = serde_json::from_str(&stdout).unwrap();
    let pin = assembled["rootSha256"].as_str().unwrap().to_string();
    let target = "extensions/a3s/registry-demo/0.1.0/stable/any/a3s-registry-demo-0.1.0-any.tar.gz";
    assert!(registry.join("targets").join(target).exists());

    let (stdout, stderr, ok) = run(binary().args([
        "withdraw-targets",
        "--registry",
        registry.to_str().unwrap(),
        "--keys-dir",
        keys.to_str().unwrap(),
        "--target",
        target,
        "--metadata-expires",
        expires,
    ]));
    assert!(ok, "withdraw-targets failed: {stderr}\n{stdout}");
    let withdrawn: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(withdrawn["rootSha256"].as_str().unwrap(), pin.as_str());
    assert_eq!(withdrawn["remainingTargets"], 0);
    assert!(!registry.join("targets").join(target).exists());

    let (stdout, stderr, ok) = run(binary().args([
        "verify",
        "--registry",
        registry.to_str().unwrap(),
        "--expected-root-sha256",
        &pin,
    ]));
    assert!(
        ok,
        "withdrawn registry must still verify: {stderr}\n{stdout}"
    );
    let report: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(report["targetsChecked"], 0);

    let (stdout, stderr, ok) = run(binary().args([
        "withdraw-targets",
        "--registry",
        registry.to_str().unwrap(),
        "--keys-dir",
        keys.to_str().unwrap(),
        "--target",
        target,
    ]));
    assert!(!ok, "second withdraw of the same target must fail closed");
    assert!(
        stderr.contains("registry_tools.withdraw_failed")
            || stdout.contains("registry_tools.withdraw_failed"),
        "stderr={stderr}\nstdout={stdout}"
    );
}
