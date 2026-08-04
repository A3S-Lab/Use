use std::path::PathBuf;

use sha2::{Digest, Sha256};

use a3s_use_core::OkfFormatVersion;

use super::{
    ExtensionManifest, PluginFlowEngine, PluginFlowRuntime, PluginMcpLaunch, SurfaceActivation,
    ToolTaskSource, ToolWorkload,
};

const NAMED_SURFACE_MANIFEST_BYTES: &[u8] = include_bytes!("../fixtures/manifests/plugin-v3.acl");
const NAMED_SURFACE_MANIFEST: &str = include_str!("../fixtures/manifests/plugin-v3.acl");
const NAMED_SURFACE_MANIFEST_DIGEST: &str =
    include_str!("../fixtures/manifests/plugin-v3.sha256").trim_ascii_end();
const OKF_MANIFEST_BYTES: &[u8] = include_bytes!("../fixtures/manifests/plugin-v3-okf.acl");
const OKF_MANIFEST: &str = include_str!("../fixtures/manifests/plugin-v3-okf.acl");
const OKF_MANIFEST_DIGEST: &str =
    include_str!("../fixtures/manifests/plugin-v3-okf.sha256").trim_ascii_end();
const COGNITIVE_MANIFEST: &str =
    include_str!("../fixtures/packages/plugin-v3-cognitive/package/a3s-use-extension.acl");

#[test]
fn schema_v3_acl_fixture_has_a_stable_digest() {
    let digest = format!("sha256:{:x}", Sha256::digest(NAMED_SURFACE_MANIFEST_BYTES));

    assert_eq!(digest, NAMED_SURFACE_MANIFEST_DIGEST);
}

#[test]
fn schema_v3_okf_acl_fixture_has_a_stable_digest() {
    let digest = format!("sha256:{:x}", Sha256::digest(OKF_MANIFEST_BYTES));

    assert_eq!(digest, OKF_MANIFEST_DIGEST);
}

#[test]
fn parses_schema_v3_named_multi_surfaces() {
    let manifest = ExtensionManifest::parse_acl(NAMED_SURFACE_MANIFEST).unwrap();

    assert_eq!(manifest.schema_version, 3);
    assert!(manifest.cli.is_none());
    assert!(manifest.mcp.is_none());
    assert!(manifest.skill.is_none());
    assert_eq!(manifest.tools.len(), 2);
    assert_eq!(manifest.mcp_servers.len(), 2);
    assert!(manifest.flows.is_empty());
    assert_eq!(manifest.skills.len(), 2);
    assert_eq!(manifest.ui.len(), 2);
    assert_eq!(manifest.surface_kinds(), ["tool", "mcp", "skill", "ui"]);

    let task = &manifest.tools[0];
    assert_eq!(task.id, "convert");
    assert_eq!(task.activation, SurfaceActivation::Lazy);
    assert!(!task.optional);
    let ToolWorkload::Task(task) = &task.workload else {
        panic!("convert must be a Tool Task");
    };
    assert_eq!(task.command, "acme-research-convert");
    assert_eq!(task.timeout_ms, 120_000);
    assert!(task.json_output);
    assert!(!task.interactive);
    assert_eq!(
        task.source,
        ToolTaskSource::Executable {
            executable: PathBuf::from("tools/convert/bin/convert")
        }
    );

    let service = &manifest.tools[1];
    assert_eq!(service.id, "index");
    assert_eq!(service.activation, SurfaceActivation::Eager);
    let ToolWorkload::Service(service) = &service.workload else {
        panic!("index must be a Tool Service");
    };
    assert_eq!(
        service.release,
        PathBuf::from("releases/index-tool-v1.json")
    );
    assert_eq!(service.base_path, "/api");
    assert_eq!(
        service.contract,
        Some(PathBuf::from("tools/index/openapi.json"))
    );

    assert!(matches!(
        manifest.mcp_servers[0].launch,
        PluginMcpLaunch::Stdio { .. }
    ));
    assert!(matches!(
        manifest.mcp_servers[1].launch,
        PluginMcpLaunch::StreamableHttp { .. }
    ));
    assert_eq!(manifest.skills[0].requires_tools, ["convert", "index"]);
    assert_eq!(manifest.skills[0].requires_mcp, ["library"]);
    assert_eq!(manifest.ui[0].skill.as_deref(), Some("review"));
    assert_eq!(manifest.ui[0].title, "review");
    assert_eq!(manifest.ui[0].description, "");
    assert_eq!(manifest.ui[0].icon, "package");
    assert_eq!(manifest.ui[0].order, 100);
    assert_eq!(manifest.ui[0].bind_tools, ["index"]);
    assert_eq!(manifest.ui[0].bind_mcp, ["library"]);
}

#[test]
fn schema_v3_parses_a3s_flow_surfaces_and_typed_consumers() {
    let manifest = ExtensionManifest::parse_acl(COGNITIVE_MANIFEST).unwrap();
    assert_eq!(
        manifest.surface_kinds(),
        ["tool", "mcp", "okf", "flow", "skill", "ui"]
    );
    let flow = &manifest.flows[0];
    assert_eq!(flow.id, "reason");
    assert_eq!(flow.engine, PluginFlowEngine::A3sFlow);
    assert_eq!(flow.runtime, PluginFlowRuntime::NativeTs);
    assert_eq!(flow.source, PathBuf::from("flows/reason.ts"));
    assert_eq!(flow.export_name, "run");
    assert_eq!(flow.requires_tools, ["echo"]);
    assert_eq!(flow.requires_mcp, ["context"]);
    assert_eq!(flow.requires_okf, ["domain"]);
    assert_eq!(manifest.skills[0].requires_flows, ["reason"]);
    assert_eq!(manifest.ui[0].bind_flows, ["reason"]);

    let graph = manifest.plugin_surfaces().unwrap();
    let flow_ref = a3s_use_core::PluginSurfaceRef {
        kind: a3s_use_core::PluginSurfaceKind::Flow,
        id: "reason".to_string(),
    };
    assert!(graph.iter().any(|surface| surface.surface == flow_ref));
    assert!(graph
        .iter()
        .find(|surface| surface.surface.kind == a3s_use_core::PluginSurfaceKind::Skill)
        .unwrap()
        .dependencies
        .contains(&flow_ref));
}

#[test]
fn schema_v3_rejects_invalid_flow_runtime_source_export_and_dependencies() {
    let unsupported_engine = COGNITIVE_MANIFEST.replace("a3s-flow", "other-flow");
    assert!(ExtensionManifest::parse_acl(&unsupported_engine)
        .unwrap_err()
        .message
        .contains("engine 'other-flow' is unsupported"));

    let unsupported = COGNITIVE_MANIFEST.replace("native-ts", "javascript");
    assert!(ExtensionManifest::parse_acl(&unsupported)
        .unwrap_err()
        .message
        .contains("runtime 'javascript' is unsupported"));

    let wrong_source = COGNITIVE_MANIFEST.replace("flows/reason.ts", "flows/reason.js");
    assert!(ExtensionManifest::parse_acl(&wrong_source)
        .unwrap_err()
        .message
        .contains("TypeScript .ts file"));

    let invalid_export =
        COGNITIVE_MANIFEST.replace("export        = \"run\"", "export = \"run-flow\"");
    assert!(ExtensionManifest::parse_acl(&invalid_export)
        .unwrap_err()
        .message
        .contains("portable TypeScript identifier"));

    let missing = COGNITIVE_MANIFEST.replace(
        "requires_tool = [\"echo\"]",
        "requires_tool = [\"missing\"]",
    );
    assert!(ExtensionManifest::parse_acl(&missing)
        .unwrap_err()
        .message
        .contains("Flow 'reason' requires unknown Tool 'missing'"));
}

#[test]
fn schema_v3_ui_parses_bounded_workbench_metadata() {
    let manifest = NAMED_SURFACE_MANIFEST.replace(
        "ui \"review\" {",
        "ui \"review\" {\n    title       = \"Research Review\"\n    description = \"Inspect evidence from the installed cognitive package.\"\n    icon        = \"flask-conical\"\n    order       = 80",
    );
    let manifest = ExtensionManifest::parse_acl(&manifest).unwrap();
    let ui = &manifest.ui[0];
    assert_eq!(ui.title, "Research Review");
    assert_eq!(
        ui.description,
        "Inspect evidence from the installed cognitive package."
    );
    assert_eq!(ui.icon, "flask-conical");
    assert_eq!(ui.order, 80);

    let invalid = NAMED_SURFACE_MANIFEST.replace(
        "ui \"review\" {",
        "ui \"review\" {\n    icon = \"Flask Icon\"",
    );
    assert!(ExtensionManifest::parse_acl(&invalid).is_err());
}

#[test]
fn rejects_duplicate_or_missing_named_surface_dependencies() {
    let duplicate = NAMED_SURFACE_MANIFEST.replace("tool \"index\" {", "tool \"convert\" {");
    let error = ExtensionManifest::parse_acl(&duplicate).unwrap_err();
    assert!(error
        .message
        .contains("Duplicate Tool surface ID 'convert'"));

    let missing_tool =
        NAMED_SURFACE_MANIFEST.replace("[\"convert\", \"index\"]", "[\"convert\", \"missing\"]");
    let error = ExtensionManifest::parse_acl(&missing_tool).unwrap_err();
    assert!(error
        .message
        .contains("Skill 'review' requires unknown Tool 'missing'"));

    let missing_skill =
        NAMED_SURFACE_MANIFEST.replace("skill     = \"review\"", "skill = \"missing\"");
    let error = ExtensionManifest::parse_acl(&missing_skill).unwrap_err();
    assert!(error
        .message
        .contains("UI 'review' requires unknown Skill 'missing'"));
}

#[test]
fn rejects_legacy_mixing_and_unsafe_schema_v3_tool_contracts() {
    let legacy_cli = r#"

  cli {
    executable = "bin/legacy"
  }
"#;
    let mixed = NAMED_SURFACE_MANIFEST.replace(
        "\n  tool \"convert\" {",
        &format!("{legacy_cli}\n  tool \"convert\" {{"),
    );
    let error = ExtensionManifest::parse_acl(&mixed).unwrap_err();
    assert!(error
        .message
        .contains("Schema version 3 cannot declare legacy"));

    let interactive = NAMED_SURFACE_MANIFEST.replace("interactive = false", "interactive = true");
    let error = ExtensionManifest::parse_acl(&interactive).unwrap_err();
    assert!(error.message.contains("Tool Tasks must be non-interactive"));

    let escaping = NAMED_SURFACE_MANIFEST.replace("tools/convert/bin/convert", "../bin/convert");
    assert!(ExtensionManifest::parse_acl(&escaping).is_err());
}

#[test]
fn schema_v3_requires_an_explicit_v3_host_compatibility_gate() {
    let current_host = NAMED_SURFACE_MANIFEST.replace(">=0.3.0, <0.4.0", ">=0.2.0, <0.4.0");
    let error = ExtensionManifest::parse_acl(&current_host).unwrap_err();
    assert!(error
        .message
        .contains("Schema version 3 must require A3S Use 0.3"));
}

#[test]
fn schema_v3_parses_an_exact_named_okf_surface_and_skill_dependency() {
    let manifest = ExtensionManifest::parse_acl(OKF_MANIFEST).unwrap();
    assert_eq!(manifest.surface_kinds(), ["okf", "skill"]);
    assert_eq!(manifest.okf.len(), 1);
    assert_eq!(manifest.okf[0].id, "domain-knowledge");
    assert_eq!(
        manifest.okf[0].bundle.format_version,
        OkfFormatVersion::V0_2
    );
    assert_eq!(manifest.okf[0].bundle.root, "okf/domain-knowledge");
    assert_eq!(
        manifest.skills[0].requires_okf,
        ["domain-knowledge".to_owned()]
    );
}

#[test]
fn schema_v3_okf_rejects_drift_unknown_authority_and_missing_dependencies() {
    let okf = r#"

  okf "domain-knowledge" {
    format_version         = "0.2"
    root                   = "okf/domain-knowledge"
    content_digest         = "sha256:bd85b0b63adb32bdf616384a619286af4c32401542655dd09e00450902ab478d"
    concept_count          = 4
    file_count             = 7
    expanded_bytes         = 2053
    max_files              = 256
    max_concepts           = 64
    max_expanded_bytes     = 67108864
    max_document_bytes     = 1048576
    max_links_per_document = 2048
    optional               = false
  }
"#;
    let manifest = NAMED_SURFACE_MANIFEST.replace(
        "\n  tool \"convert\" {",
        &format!("{okf}\n  tool \"convert\" {{"),
    );

    let unknown_dependency = manifest.replace(
        "requires_mcp  = [\"library\"]",
        "requires_mcp  = [\"library\"]\n    requires_okf  = [\"missing\"]",
    );
    let error = ExtensionManifest::parse_acl(&unknown_dependency).unwrap_err();
    assert!(error
        .message
        .contains("requires unknown OKF surface 'missing'"));

    let executable_metadata = manifest.replace(
        "optional               = false",
        "optional               = false\n    executor               = \"tool:index\"",
    );
    let error = ExtensionManifest::parse_acl(&executable_metadata).unwrap_err();
    assert!(error.message.contains("Unknown 'okf' attribute 'executor'"));

    let escaping = manifest.replace("okf/domain-knowledge", "../personal-vault");
    assert!(ExtensionManifest::parse_acl(&escaping).is_err());
}

#[test]
fn schema_v3_parses_sorted_versioned_package_dependencies() {
    let dependencies = r#"

  dependency "acme/vector-store" {
    version = "^2.1.0"
  }

  dependency "acme/base" {
    version = ">=1.0.0, <2.0.0"
  }
"#;
    let manifest = NAMED_SURFACE_MANIFEST.replace(
        "\n  repository {",
        &format!("{dependencies}\n  repository {{"),
    );
    let parsed = ExtensionManifest::parse_acl(&manifest).unwrap();
    assert_eq!(parsed.dependencies.len(), 2);
    assert_eq!(parsed.dependencies[0].package_id, "acme/base");
    assert_eq!(
        parsed.dependencies[0].version_requirement,
        ">=1.0.0, <2.0.0"
    );
    assert_eq!(parsed.dependencies[1].package_id, "acme/vector-store");
    assert_eq!(parsed.dependencies[1].version_requirement, "^2.1.0");
}

#[test]
fn schema_v3_rejects_unsafe_package_dependency_declarations() {
    let dependency = r#"

  dependency "acme/base" {
    version = "^1.0.0"
  }
"#;
    let manifest = NAMED_SURFACE_MANIFEST.replace(
        "\n  repository {",
        &format!("{dependency}\n  repository {{"),
    );

    let duplicate = manifest.replace(dependency, &format!("{dependency}{dependency}"));
    assert!(ExtensionManifest::parse_acl(&duplicate)
        .unwrap_err()
        .message
        .contains("sorted and unique"));

    let self_dependency = manifest.replace("acme/base", "acme/research");
    assert!(ExtensionManifest::parse_acl(&self_dependency)
        .unwrap_err()
        .message
        .contains("cannot depend on itself"));

    let noncanonical = manifest.replace("^1.0.0", "1.0.0");
    assert!(ExtensionManifest::parse_acl(&noncanonical)
        .unwrap_err()
        .message
        .contains("canonical semantic-version"));

    let endpoint = manifest.replace(
        "version = \"^1.0.0\"",
        "version = \"^1.0.0\"\n    registry = \"https://untrusted.example\"",
    );
    assert!(ExtensionManifest::parse_acl(&endpoint)
        .unwrap_err()
        .message
        .contains("Unknown 'dependency' attribute 'registry'"));
}
