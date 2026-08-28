#![cfg(feature = "extensions")]

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::process::{Command, Output};
use std::time::{Duration, Instant};

use a3s_use::cognitive_package::{
    CognitivePackageEnablementPlanResult, CognitivePackageEnablementPlanStatus,
    CognitivePackageEnablementRequest, CognitivePackageManager,
};
use a3s_use_core::{
    CatalogAvailability, CatalogSurface, PluginCatalogRecord, PluginDesiredState,
    PluginObservedState, PluginOperationAction, PluginPackageDependency, PluginPackageLockHost,
    PluginReleaseChannel, PluginSurfaceKind, PLUGIN_CATALOG_SCHEMA_V3,
};
use a3s_use_extension::{
    prepare_remote_package, resolve_remote_package_lock, ExtensionPaths, ExtensionRegistry,
    ResolvedRemotePackage, TrustedRegistry,
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

#[cfg(any(unix, windows))]
#[path = "remote_extension_cli/archive_recovery.rs"]
mod archive_recovery;
#[cfg(any(unix, windows))]
#[path = "remote_extension_cli/flow_lifecycle.rs"]
mod flow_lifecycle;
#[cfg(any(unix, windows))]
#[path = "remote_extension_cli/grant_process_recovery.rs"]
mod grant_process_recovery;
#[path = "remote_extension_cli/graph_grants.rs"]
mod graph_grants;
#[path = "remote_extension_cli/graph_install.rs"]
mod graph_install;
#[path = "remote_extension_cli/graph_mutation_recovery.rs"]
mod graph_mutation_recovery;
#[path = "remote_extension_cli/graph_mutation_serialization.rs"]
mod graph_mutation_serialization;
#[cfg(any(unix, windows))]
#[path = "remote_extension_cli/graph_recovery.rs"]
mod graph_recovery;
#[path = "remote_extension_cli/graph_upgrade.rs"]
mod graph_upgrade;
#[path = "remote_extension_cli/operation_diagnostic.rs"]
mod operation_diagnostic;
#[cfg(any(unix, windows))]
#[path = "remote_extension_cli/planning_target_diagnostic.rs"]
mod planning_target_diagnostic;
#[path = "remote_extension_cli/recovery.rs"]
mod recovery;
#[path = "remote_extension_cli/registry.rs"]
mod registry;
#[path = "remote_extension_cli/registry_cache.rs"]
mod registry_cache;
#[path = "remote_extension_cli/resolution_diagnostic.rs"]
mod resolution_diagnostic;

fn registry_install(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    package_lock_digest: Option<&str>,
    extra: &[&str],
) -> Output {
    configure_registry(server, repository, home, &[]);
    let mut command = Command::new(binary());
    command.args([
        "component",
        "install",
        "a3s/science",
        "--registry-name",
        "fixture",
    ]);
    if let Some(package_lock_digest) = package_lock_digest {
        command.args(["--package-lock-digest", package_lock_digest]);
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
    configure_registry(server, repository, home, &[]);
    Command::new(binary())
        .args([
            "install",
            package_id,
            "--registry-name",
            "fixture",
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

fn cognitive_okf_target(
    fixture_root: &std::path::Path,
    version: &str,
    decision: &str,
    target: &str,
) -> TestTarget {
    let source = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("crates/extension/fixtures/packages/plugin-v3-okf/package");
    let package_root = fixture_root.join("package");
    copy_fixture_tree(&source, &package_root);

    let decision_path = package_root.join("okf/domain-knowledge/concepts/package-lifecycle.md");
    let original = std::fs::read_to_string(&decision_path).unwrap();
    let body_start = original.find("# Decision").unwrap();
    let frontmatter = &original[..body_start];
    std::fs::write(
        &decision_path,
        format!("{frontmatter}# Decision\n\n{decision}\n"),
    )
    .unwrap();

    let okf_root = package_root.join("okf/domain-knowledge");
    let mut files = Vec::new();
    collect_okf_files(&okf_root, &okf_root, &mut files);
    files.sort_by(|left, right| left.path.cmp(&right.path));
    let limits = a3s_use_core::OkfBundleLimits {
        max_files: 256,
        max_concepts: 64,
        max_expanded_bytes: 67_108_864,
        max_document_bytes: 1_048_576,
        max_links_per_document: 2_048,
    };
    let inspection = a3s_use_core::inspect_okf_bundle_files(
        a3s_use_core::OkfFormatVersion::V0_2,
        limits,
        &files,
    )
    .unwrap();

    let manifest_path = package_root.join("a3s-use-extension.acl");
    let manifest = std::fs::read_to_string(&manifest_path)
        .unwrap()
        .replace(
            "version        = \"1.0.0\"",
            &format!("version        = \"{version}\""),
        )
        .replace(
            "sha256:bd85b0b63adb32bdf616384a619286af4c32401542655dd09e00450902ab478d",
            &inspection.content_digest,
        )
        .replace(
            "expanded_bytes         = 2053",
            &format!("expanded_bytes         = {}", inspection.expanded_bytes),
        );
    std::fs::write(&manifest_path, &manifest).unwrap();
    let parsed = a3s_use_extension::ExtensionManifest::parse_acl(&manifest).unwrap();

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.version = version.to_string();
    catalog.target = target.to_string();
    catalog.surfaces[0].okf_bundle = Some(parsed.okf[0].bundle.clone());
    catalog.archive.target_name = format!(
        "extensions/acme/knowledge/{version}/stable/{target}/acme-knowledge-{version}-{target}.tar.gz"
    );
    catalog.archive.length = archive.len() as u64;
    catalog.archive.sha256 = format!("sha256:{:x}", Sha256::digest(&archive));
    catalog.package.expanded_bytes = fingerprint.2;
    catalog.package.file_count = fingerprint.1;
    catalog.package.sha256 = Some(format!("sha256:{}", fingerprint.0));
    catalog.package.manifest_sha256 =
        Some(format!("sha256:{:x}", Sha256::digest(manifest.as_bytes())));
    catalog.validate().unwrap();

    TestTarget {
        target_name: catalog.archive.target_name.clone(),
        custom: Some(serde_json::to_value(catalog).unwrap()),
        archive,
    }
}

fn copy_fixture_tree(source: &std::path::Path, destination: &std::path::Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_fixture_tree(&source_path, &destination_path);
        } else {
            std::fs::copy(source_path, destination_path).unwrap();
        }
    }
}

fn collect_okf_files(
    root: &std::path::Path,
    directory: &std::path::Path,
    files: &mut Vec<a3s_use_core::OkfBundleFile>,
) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            collect_okf_files(root, &path, files);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            files.push(a3s_use_core::OkfBundleFile::new(
                relative,
                std::fs::read(path).unwrap(),
            ));
        }
    }
}

fn cognitive_registry_upgrade(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    package_id: &str,
    version: &str,
    extra: &[&str],
) -> Output {
    configure_registry(server, repository, home, &[]);
    Command::new(binary())
        .args([
            "upgrade",
            package_id,
            "--registry-name",
            "fixture",
            "--version",
            version,
        ])
        .args(extra)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap()
}

fn configure_registry(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
    policy: &[&str],
) {
    if home.join("state/registries.acl").exists() {
        return;
    }
    let configured = Command::new(binary())
        .args([
            "registry",
            "source",
            "add",
            "fixture",
            "--url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
        ])
        .args(policy)
        .arg("--json")
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap();
    assert!(configured.status.success(), "{configured:?}");
}

fn registry_source_snapshot(home: &std::path::Path) -> serde_json::Value {
    let listed = Command::new(binary())
        .args(["registry", "source", "list", "--json"])
        .env("A3S_USE_HOME", home)
        .output()
        .unwrap();
    assert!(listed.status.success(), "{listed:?}");
    json(&listed)["data"]["registrySources"].clone()
}

fn replace_registry(
    server: &TestServer,
    repository: &TestRepository,
    home: &std::path::Path,
) -> Output {
    let revision = registry_source_snapshot(home)["revision"]
        .as_str()
        .unwrap()
        .to_owned();
    Command::new(binary())
        .args([
            "registry",
            "source",
            "replace",
            "fixture",
            "--url",
            server.base_url(),
            "--trust-root",
            &repository.root_sha256,
            "--expected-revision",
            &revision,
            "--yes",
            "--json",
        ])
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
        .join("user")
        .join(scope)
        .join(package_id)
        .join("active.json")
}

async fn apply_planned_enablement(
    manager: &CognitivePackageManager,
    request: &CognitivePackageEnablementRequest,
) -> a3s_use_core::UseResult<a3s_use::cognitive_package::CognitivePackageEnablementResult> {
    let mut planned = Box::new(Box::pin(manager.plan_enablement(request)).await?);
    if let Some(result) = planned.result.take() {
        return Ok(result);
    }
    let envelope = planned.plan.take().ok_or_else(|| {
        a3s_use_core::UseError::new(
            "test.plugin.enablement_plan_missing",
            "The integration test expected an enablement plan to apply.",
        )
    })?;
    let confirmation = (envelope.plan.authority.decision == a3s_use_core::PlanPolicyDecision::Ask)
        .then(|| a3s_use_core::PluginOperationConfirmation {
            schema: a3s_use_core::PLUGIN_OPERATION_CONFIRMATION_SCHEMA.to_string(),
            operation_id: envelope.plan.operation_id.clone(),
            plan_digest: envelope.plan_digest.clone(),
            confirmed_by: a3s_use_core::PlanActor::User,
            confirmed_at_ms: envelope.plan.created_at_ms + 1,
        });
    Box::pin(manager.apply_enablement(request, envelope, confirmation)).await
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

#[cfg(any(unix, windows))]
fn cognitive_flow_target_version(
    fixture_root: &std::path::Path,
    version: &str,
    target: &str,
) -> TestTarget {
    let package_id = "acme/flow-suite";
    let route = "flow-suite";
    let package_root = fixture_root.join("package");
    std::fs::create_dir_all(package_root.join("flows")).unwrap();
    std::fs::create_dir_all(package_root.join("skills/reason")).unwrap();
    std::fs::create_dir_all(package_root.join("ui/reason")).unwrap();
    std::fs::create_dir_all(package_root.join("okf/domain/concepts")).unwrap();

    let manifest = format!(
        r#"extension "{package_id}" {{
  schema_version = 3
  version        = "{version}"
  route          = "{route}"
  requires_use   = ">=0.3.0, <0.4.0"
  actions        = ["read"]

  repository {{
    url      = "https://github.com/acme/flow-suite"
    revision = "0123456789abcdef0123456789abcdef01234567"
  }}

  okf "domain" {{
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
  }}

  flow "reason" {{
    engine        = "a3s-flow"
    runtime       = "native-ts"
    source        = "flows/reason.ts"
    export        = "run"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = ["domain"]
    optional      = false
  }}

  skill "reason" {{
    path          = "skills/reason/SKILL.md"
    requires_tool = []
    requires_mcp  = []
    requires_okf  = ["domain"]
    requires_flow = ["reason"]
    optional      = false
  }}

  ui "reason" {{
    entry     = "ui/reason/index.html"
    styles    = ["ui/reason/index.css"]
    scripts   = ["ui/reason/index.js"]
    skill     = "reason"
    bind_tool = []
    bind_mcp  = []
    bind_flow = ["reason"]
    optional  = false
  }}
}}
"#
    );
    std::fs::write(package_root.join("a3s-use-extension.acl"), &manifest).unwrap();
    std::fs::write(
        package_root.join("README.md"),
        format!("# Flow Suite {version}\n\nStandalone Flow lifecycle fixture.\n"),
    )
    .unwrap();
    std::fs::write(
        package_root.join("flows/reason.ts"),
        format!(
            "export function run() {{ return {{ type: 'complete', output: {{ version: '{version}' }} }}; }}\n"
        ),
    )
    .unwrap();
    std::fs::write(
        package_root.join("skills/reason/SKILL.md"),
        "---\nname: reason\ndescription: Exercise the installed Flow and Knowledge surfaces\n---\n# Reason\n",
    )
    .unwrap();
    std::fs::write(
        package_root.join("ui/reason/index.html"),
        "<!doctype html><html><body><main id=\"app\">Reason</main></body></html>\n",
    )
    .unwrap();
    std::fs::write(
        package_root.join("ui/reason/index.css"),
        "body { font-family: sans-serif; }\n",
    )
    .unwrap();
    std::fs::write(
        package_root.join("ui/reason/index.js"),
        "document.querySelector('#app').dataset.ready = 'true';\n",
    )
    .unwrap();
    std::fs::write(
        package_root.join("okf/domain/index.md"),
        include_bytes!(
            "../crates/extension/fixtures/packages/plugin-v3-cognitive/package/okf/domain/index.md"
        ),
    )
    .unwrap();
    std::fs::write(
        package_root.join("okf/domain/concepts/lifecycle.md"),
        include_bytes!(
            "../crates/extension/fixtures/packages/plugin-v3-cognitive/package/okf/domain/concepts/lifecycle.md"
        ),
    )
    .unwrap();

    let archive = package_directory_archive(&package_root);
    let fingerprint = package_fingerprint(&package_root);
    let manifest_sha256 = format!("sha256:{:x}", Sha256::digest(manifest.as_bytes()));
    let parsed = a3s_use_extension::ExtensionManifest::parse_acl(&manifest).unwrap();
    let graph = parsed.plugin_surfaces().unwrap();
    let mut catalog = PluginCatalogRecord::from_json(OKF_CATALOG_V3).unwrap();
    catalog.schema = PLUGIN_CATALOG_SCHEMA_V3.to_string();
    catalog.package_id = package_id.to_string();
    catalog.display_name = format!("Flow Suite {version}");
    catalog.description = "Standalone A3S Flow lifecycle fixture.".to_string();
    catalog.publisher = "acme".to_string();
    catalog.keywords = vec!["fixture".to_string(), "flow".to_string()];
    catalog.categories = vec!["workflow".to_string()];
    catalog.version = version.to_string();
    catalog.channel = PluginReleaseChannel::Stable;
    catalog.requires_use = ">=0.3.0, <0.4.0".to_string();
    catalog.dependencies.clear();
    catalog.target = target.to_string();
    catalog.surfaces = graph
        .iter()
        .map(|surface| CatalogSurface {
            kind: surface.surface.kind,
            id: surface.surface.id.clone(),
            optional: surface.optional,
            workload: None,
            mcp_transport: None,
            mcp_tool_count: None,
            okf_bundle: parsed
                .okf
                .iter()
                .find(|okf| {
                    surface.surface.kind == PluginSurfaceKind::Okf && okf.id == surface.surface.id
                })
                .map(|okf| okf.bundle.clone()),
            requires: surface.dependencies.clone(),
        })
        .collect();
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
    catalog.repository = "https://github.com/acme/flow-suite".to_string();
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
