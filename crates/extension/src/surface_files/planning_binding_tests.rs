use a3s_use_core::{
    ExecutablePlanningSurface, PlanningSurfaceActivation, PluginPlanningBundle,
    PluginReleaseChannel, PLUGIN_PLANNING_BUNDLE_SCHEMA,
};

use super::*;

#[tokio::test]
async fn native_planning_launchers_bind_exactly_to_the_package_manifest() {
    let manifest = ExtensionManifest::parse_acl(
        r#"
extension "a3s/science" {
  schema_version = 3
  version = "0.2.2"
  route = "science"
  requires_use = ">=0.3.0, <0.4.0"
  actions = ["read"]

  repository {
    url = "https://github.com/A3S-Lab/Science"
  }

  tool "science" {
    workload = "task"
    interface = "cli"
    executable = "bin/science-fixture"
    command = "science-fixture"
    json_output = true
    interactive = false
    timeout_ms = 120000
    activation = "lazy"
    optional = false
  }

  mcp "science" {
    transport = "stdio"
    executable = "bin/science-fixture"
    args = ["serve", "--mcp"]
    activation = "lazy"
    optional = false
  }
}
"#,
    )
    .unwrap();
    let mut bundle = PluginPlanningBundle {
        schema: PLUGIN_PLANNING_BUNDLE_SCHEMA.to_owned(),
        package_id: "a3s/science".to_owned(),
        version: "0.2.2".to_owned(),
        channel: PluginReleaseChannel::Stable,
        target: "linux-x86_64".to_owned(),
        archive_sha256: format!("sha256:{}", "a".repeat(64)),
        package_sha256: format!("sha256:{}", "b".repeat(64)),
        manifest_sha256: format!("sha256:{}", "c".repeat(64)),
        permission_ceiling_digest: format!("sha256:{}", "d".repeat(64)),
        surfaces: vec![
            ExecutablePlanningSurface::McpStdio {
                id: "science".to_owned(),
                activation: PlanningSurfaceActivation::Lazy,
                executable: "bin/science-fixture".to_owned(),
                args: vec!["serve".to_owned(), "--mcp".to_owned()],
            },
            ExecutablePlanningSurface::ToolTaskNative {
                id: "science".to_owned(),
                activation: PlanningSurfaceActivation::Lazy,
                executable: "bin/science-fixture".to_owned(),
                command: "science-fixture".to_owned(),
                json_output: true,
                timeout_ms: 120_000,
            },
        ],
    };
    let package = tempfile::tempdir().unwrap();

    validate_planning_bundle_package_binding(&bundle, &manifest, package.path())
        .await
        .unwrap();

    let ExecutablePlanningSurface::McpStdio { args, .. } = &mut bundle.surfaces[0] else {
        panic!("first planning surface should be stdio MCP");
    };
    args.push("--different".to_owned());
    let error = validate_planning_bundle_package_binding(&bundle, &manifest, package.path())
        .await
        .unwrap_err();
    assert_eq!(error.code, "use.extension.planning_package_mismatch");
}
