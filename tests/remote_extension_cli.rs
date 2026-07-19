#![cfg(feature = "extensions")]

use std::process::{Command, Output};

use a3s_use_extension::{prepare_remote_package, ResolvedRemotePackage, TrustedRegistry};

#[path = "../crates/extension/src/tuf_test_support.rs"]
mod tuf_test_support;

use tuf_test_support::{extension_archive, TestRepository, TestServer, FUTURE, PACKAGE_VERSION};

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
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
    let reviewed = prepare_remote_package(&trusted, "acme/slack", None, "stable", None)
        .await
        .unwrap();
    let plan_digest = reviewed.resolved().plan_digest().unwrap();
    drop(reviewed);
    assert_no_target_request(&server);

    let home = temp.path().join("home");
    let installed = registry_install(&server, &repository, &home, Some(&plan_digest), &[]);
    assert!(installed.status.success(), "{installed:?}");
    let installed_json = json(&installed);
    assert_eq!(installed_json["data"]["changed"], true);
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
        &std::fs::read(home.join("state/extensions/acme/slack.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(receipt["trust"], "registry-tuf");
    let provenance: ResolvedRemotePackage =
        serde_json::from_value(receipt["registry"].clone()).unwrap();
    assert_eq!(provenance.plan_digest().unwrap(), plan_digest);

    let inspected = Command::new(binary())
        .args(["extension", "inspect", "acme/slack", "--json"])
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

    let second = registry_install(&server, &repository, &home, Some(&plan_digest), &[]);
    assert!(second.status.success(), "{second:?}");
    assert_eq!(json(&second)["data"]["changed"], false);
}

#[test]
fn registry_plan_mismatch_fails_before_target_download() {
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
        "use.extension.registry_plan_mismatch"
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
            "acme/slack",
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

fn registry_install(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    plan_digest: Option<&str>,
    extra: &[&str],
) -> Output {
    let mut command = Command::new(binary());
    command.args([
        "component",
        "install",
        "acme/slack",
        "--registry-name",
        "fixture",
        "--registry-url",
        server.base_url(),
        "--trust-root",
        &repository.root_sha256,
    ]);
    if let Some(plan_digest) = plan_digest {
        command.args(["--registry-plan-digest", plan_digest]);
    }
    command
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|error| {
        panic!(
            "invalid JSON output ({error}): stdout={:?}, stderr={:?}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    })
}

fn assert_no_target_request(server: &TestServer) {
    assert!(server
        .requests()
        .iter()
        .all(|request| !request.starts_with("/targets/")));
}
