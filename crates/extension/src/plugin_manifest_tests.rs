use std::path::PathBuf;

use sha2::{Digest, Sha256};

use super::{ExtensionManifest, PluginMcpLaunch, SurfaceActivation, ToolTaskSource, ToolWorkload};

const NAMED_SURFACE_MANIFEST_BYTES: &[u8] = include_bytes!("../fixtures/manifests/plugin-v3.acl");
const NAMED_SURFACE_MANIFEST: &str = include_str!("../fixtures/manifests/plugin-v3.acl");
const NAMED_SURFACE_MANIFEST_DIGEST: &str =
    include_str!("../fixtures/manifests/plugin-v3.sha256").trim_ascii_end();

#[test]
fn schema_v3_acl_fixture_has_a_stable_digest() {
    let digest = format!("sha256:{:x}", Sha256::digest(NAMED_SURFACE_MANIFEST_BYTES));

    assert_eq!(digest, NAMED_SURFACE_MANIFEST_DIGEST);
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
    assert_eq!(manifest.ui[0].bind_tools, ["index"]);
    assert_eq!(manifest.ui[0].bind_mcp, ["library"]);
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
fn schema_v3_keeps_okf_fail_closed_until_the_full_m0k_contract_is_wired() {
    let okf = r#"

  okf "domain-knowledge" {
    format_version = "0.2"
    root           = "okf/domain-knowledge"
  }
"#;
    let manifest = NAMED_SURFACE_MANIFEST.replace(
        "\n  tool \"convert\" {",
        &format!("{okf}\n  tool \"convert\" {{"),
    );

    let error = ExtensionManifest::parse_acl(&manifest).unwrap_err();
    assert!(error.message.contains("Unknown extension surface 'okf'"));
}
