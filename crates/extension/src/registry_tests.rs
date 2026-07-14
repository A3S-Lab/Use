use std::os::unix::fs::PermissionsExt;

use super::*;

const MANIFEST_NAME: &str = "a3s-use-extension.acl";

async fn package(root: &Path, package_id: &str, route: &str, version: &str) {
    fs::create_dir_all(root.join("bin")).await.unwrap();
    fs::create_dir_all(root.join("skills/demo")).await.unwrap();
    let manifest = format!(
        r#"extension "{package_id}" {{
  schema_version = 1
  version = "{version}"
  route = "{route}"
  actions = ["read"]

  cli {{
executable = "bin/extension"
json_output = true
  }}

  skill {{
path = "skills/demo/SKILL.md"
  }}
}}
"#
    );
    fs::write(root.join(MANIFEST_NAME), manifest).await.unwrap();
    let executable = root.join("bin/extension");
    fs::write(&executable, "#!/bin/sh\nprintf 'ok\\n'\n")
        .await
        .unwrap();
    let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&executable, permissions).unwrap();
    fs::write(root.join("skills/demo/SKILL.md"), "# Demo\n")
        .await
        .unwrap();
}

fn registry(root: &Path) -> ExtensionRegistry {
    ExtensionRegistry::new(ExtensionPaths::new(root.join("data"), root.join("state")))
}

#[tokio::test]
async fn installs_lists_and_uninstalls_an_explicit_local_package() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.2.0").await;
    let registry = registry(temp.path());

    let result = registry
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    assert!(result.changed);
    assert_eq!(result.extension.surfaces(), ["cli", "skill"]);
    assert!(result.extension.cli_executable().unwrap().is_file());
    assert_eq!(registry.list().await.unwrap().len(), 1);

    let unchanged = registry
        .install_local(
            "use/acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    assert!(!unchanged.changed);

    let removed = registry.uninstall("acme/slack").await.unwrap();
    assert!(removed.changed);
    assert!(registry.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn rejects_route_conflicts_and_untrusted_installs() {
    let temp = tempfile::tempdir().unwrap();
    let first = temp.path().join("first");
    let second = temp.path().join("second");
    package(&first, "acme/slack", "chat", "1.0.0").await;
    package(&second, "example/teams", "chat", "1.0.0").await;
    let registry = registry(temp.path());

    let error = registry
        .install_local("acme/slack", &first, InstallOptions::default())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.trust_required");

    registry
        .install_local(
            "acme/slack",
            &first,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap();
    let error = registry
        .install_local(
            "example/teams",
            &second,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.route_conflict");
}

#[tokio::test]
async fn rejects_package_symlinks() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source");
    package(&source, "acme/slack", "slack", "1.0.0").await;
    std::os::unix::fs::symlink("/etc/passwd", source.join("escape")).unwrap();
    let error = registry(temp.path())
        .install_local(
            "acme/slack",
            &source,
            InstallOptions {
                allow_unsigned: true,
                force: false,
            },
        )
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.package_symlink");
}
