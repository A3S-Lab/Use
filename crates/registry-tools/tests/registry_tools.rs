//! End-to-end tests for the registry tools binary: keygen → pack →
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
        let (stdout, stderr, ok) =
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
    fs::write(
        &manifest_path,
        manifest
            .replace("0.1.0", "0.2.0")
            .replace("skills/demo/SKILL.md", "skills/demo/SKILL.md"),
    )
    .unwrap();
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
