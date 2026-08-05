#![cfg(feature = "extensions")]

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use a3s_use::cognitive_package::CognitivePackageManager;
use a3s_use_core::{
    CatalogAvailability, CatalogSurface, PluginCatalogRecord, PluginPackageDependency,
    PluginReleaseChannel, PluginSurfaceKind, PLUGIN_CATALOG_SCHEMA_V3,
};
use a3s_use_extension::{
    prepare_remote_package, ExtensionPaths, ExtensionRegistry, ResolvedRemotePackage,
    TrustedRegistry,
};
use fs2::FileExt;
use sha2::{Digest, Sha256};

#[path = "../crates/extension/src/tuf_test_support.rs"]
mod tuf_test_support;

use tuf_test_support::{
    extension_archive, package_directory_archive, TestRepository, TestServer, TestTarget, FUTURE,
    PACKAGE_VERSION,
};

const OKF_CATALOG_V3: &[u8] =
    include_bytes!("../crates/core/fixtures/plugins/catalog-record-okf-v3.json");

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_a3s-use")
}

#[path = "remote_extension_cli/graph_grants.rs"]
mod graph_grants;
#[path = "remote_extension_cli/graph_install.rs"]
mod graph_install;
#[path = "remote_extension_cli/graph_upgrade.rs"]
mod graph_upgrade;
#[path = "remote_extension_cli/recovery.rs"]
mod recovery;
#[path = "remote_extension_cli/registry.rs"]
mod registry;

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
        "a3s/science",
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

fn cognitive_registry_install(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    package_id: &str,
    extra: &[&str],
) -> Output {
    Command::new(binary())
        .args([
            "install",
            package_id,
            "--registry-name",
            "fixture",
            "--registry-url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
            "--version",
            "1.0.0",
        ])
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn cognitive_uninstall(home: &std::path::Path, package_id: &str) -> Output {
    Command::new(binary())
        .args(["uninstall", package_id, "--json"])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn cognitive_registry_upgrade(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    package_id: &str,
    version: &str,
    extra: &[&str],
) -> Output {
    Command::new(binary())
        .args([
            "upgrade",
            package_id,
            "--registry-name",
            "fixture",
            "--registry-url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
            "--version",
            version,
        ])
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn exclusive_lock(path: &std::path::Path) -> File {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    FileExt::lock_exclusive(&file).unwrap();
    file
}

fn wait_until(timeout: Duration, condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    condition()
}

fn target_request_count(server: &TestServer) -> usize {
    server
        .requests()
        .iter()
        .filter(|request| request.starts_with("/targets/"))
        .count()
}

fn lifecycle_journal_path(home: &std::path::Path, package_id: &str) -> std::path::PathBuf {
    let scope = format!("{:x}", Sha256::digest(b"user/current"));
    home.join("state/operations/plugins")
        .join(scope)
        .join(package_id)
        .join("active.json")
}

fn cognitive_skill_target(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    dependencies: Vec<PluginPackageDependency>,
    target: &str,
) -> TestTarget {
    cognitive_skill_target_version(
        fixture_root,
        package_id,
        route,
        "1.0.0",
        dependencies,
        target,
    )
}

fn cognitive_skill_target_version(
    fixture_root: &std::path::Path,
    package_id: &str,
    route: &str,
    version: &str,
    dependencies: Vec<PluginPackageDependency>,
    target: &str,
) -> TestTarget {
    let package_root = fixture_root.join("packages").join(route);
    std::fs::create_dir_all(package_root.join("skills/main")).unwrap();
    let dependency_blocks = dependencies
        .iter()
        .map(|dependency| {
            format!(
                "\n  dependency \"{}\" {{\n    version = \"{}\"\n  }}\n",
                dependency.package_id, dependency.version_requirement
            )
        })
        .collect::<String>();
    let manifest = format!(
        "extension \"{package_id}\" {{\n  schema_version = 3\n  version = \"{version}\"\n  route = \"{route}\"\n  requires_use = \">=0.3.0, <0.4.0\"\n  actions = [\"read\"]\n{dependency_blocks}\n  repository {{\n    url = \"https://github.com/acme/{route}\"\n    revision = \"0123456789abcdef0123456789abcdef01234567\"\n  }}\n\n  skill \"main\" {{\n    path = \"skills/main/SKILL.md\"\n    requires_tool = []\n    requires_mcp = []\n    requires_okf = []\n    optional = false\n  }}\n}}\n"
    );
    std::fs::write(package_root.join("a3s-use-extension.acl"), &manifest).unwrap();
    std::fs::write(
        package_root.join("README.md"),
        format!("# {package_id}\n\nCognitive package integration fixture.\n"),
    )
    .unwrap();
    std::fs::write(
        package_root.join("skills/main/SKILL.md"),
        format!("---\nname: {route}\ndescription: Cognitive package fixture\n---\n# {route}\n"),
    )
    .unwrap();

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let manifest_sha256 = format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()));
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    catalog.package_id = package_id.to_string();
    catalog.display_name = format!("{route} fixture");
    catalog.description = format!("Cognitive package fixture for {package_id}.");
    catalog.publisher = "acme".to_string();
    catalog.keywords = vec!["fixture".to_string()];
    catalog.categories = vec!["test".to_string()];
    catalog.version = version.to_string();
    catalog.channel = PluginReleaseChannel::Stable;
    catalog.requires_use = ">=0.3.0, <0.4.0".to_string();
    catalog.dependencies = dependencies;
    catalog.target = target.to_string();
    catalog.surfaces = vec![CatalogSurface {
        kind: PluginSurfaceKind::Skill,
        id: "main".to_string(),
        optional: false,
        workload: None,
        mcp_transport: None,
        mcp_tool_count: None,
        okf_bundle: None,
        requires: Vec::new(),
    }];
    catalog.permission_ceiling.surfaces.clear();
    catalog.permission_ceiling_digest = catalog.permission_ceiling.descriptor_digest().unwrap();
    catalog.planning = None;
    catalog.archive.target_name = format!(
        "extensions/{package_id}/{version}/stable/{target}/{route}-{version}-{target}.tar.gz"
    );
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.package.manifest_sha256 = Some(manifest_sha256);
    catalog.license = "MIT".to_string();
    catalog.repository = format!("https://github.com/acme/{route}");
    catalog.availability = CatalogAvailability::Available;
    catalog.validate().unwrap();

    TestTarget {
        target_name: catalog.archive.target_name.clone(),
        custom: Some(serde_json::to_value(catalog).unwrap()),
        archive,
    }
}

fn package_fingerprint(root: &std::path::Path) -> (String, u64, u64) {
    fn collect(
        root: &std::path::Path,
        directory: &std::path::Path,
        files: &mut Vec<(String, std::path::PathBuf)>,
    ) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                collect(root, &path, files);
            } else {
                files.push((
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .replace('\\', "/"),
                    path,
                ));
            }
        }
    }

    let mut files = Vec::new();
    collect(root, root, &mut files);
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    digest.update(b"a3s-use-expanded-package-v1\0");
    let mut expanded_bytes = 0_u64;
    for (relative, path) in &files {
        let size = std::fs::metadata(path).unwrap().len();
        expanded_bytes += size;
        digest.update((relative.len() as u64).to_be_bytes());
        digest.update(relative.as_bytes());
        digest.update(size.to_be_bytes());
        let mut input = std::fs::File::open(path).unwrap();
        let mut buffer = Vec::new();
        input.read_to_end(&mut buffer).unwrap();
        digest.update(buffer);
    }
    (
        format!("{:x}", digest.finalize()),
        files.len() as u64,
        expanded_bytes,
    )
}

fn host_target() -> String {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos", "aarch64") => "darwin-arm64",
        ("macos", "x86_64") => "darwin-x86_64",
        ("linux", "aarch64") => "linux-arm64",
        ("linux", "x86_64") => "linux-x86_64",
        ("windows", "x86_64") => "windows-x86_64",
        (os, arch) => panic!("unsupported test target {os}-{arch}"),
    }
    .to_string()
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
