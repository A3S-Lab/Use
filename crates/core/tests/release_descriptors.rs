use std::collections::BTreeMap;

use a3s_use_core::{McpReleaseDescriptor, ReleaseResolution, SkillReleaseDescriptor};

const MCP_FIXTURE: &[u8] = include_bytes!("../fixtures/releases/mcp-release-v1.json");
const SKILL_FIXTURE: &[u8] = include_bytes!("../fixtures/releases/skill-release-v1.json");
const MCP_DESCRIPTOR_DIGEST: &str =
    include_str!("../fixtures/releases/mcp-release-v1.sha256").trim_ascii_end();
const SKILL_DESCRIPTOR_DIGEST: &str =
    include_str!("../fixtures/releases/skill-release-v1.sha256").trim_ascii_end();

fn canonical_fixture(bytes: &[u8]) -> &[u8] {
    bytes.strip_suffix(b"\n").unwrap_or(bytes)
}

#[test]
fn canonical_release_fixtures_have_cross_sdk_digest_goldens() {
    let mcp = McpReleaseDescriptor::from_json(MCP_FIXTURE).unwrap();
    let skill = SkillReleaseDescriptor::from_json(SKILL_FIXTURE).unwrap();

    assert_eq!(
        mcp.canonical_bytes().unwrap(),
        canonical_fixture(MCP_FIXTURE)
    );
    assert_eq!(
        skill.canonical_bytes().unwrap(),
        canonical_fixture(SKILL_FIXTURE)
    );
    assert_eq!(mcp.descriptor_digest().unwrap(), MCP_DESCRIPTOR_DIGEST);
    assert_eq!(skill.descriptor_digest().unwrap(), SKILL_DESCRIPTOR_DIGEST);

    let reordered = serde_json::to_vec_pretty(
        &serde_json::from_slice::<serde_json::Value>(MCP_FIXTURE).unwrap(),
    )
    .unwrap();
    assert_eq!(
        McpReleaseDescriptor::from_json(&reordered)
            .unwrap()
            .descriptor_digest()
            .unwrap(),
        MCP_DESCRIPTOR_DIGEST
    );
}

#[test]
fn mutable_or_orchestration_fields_fail_closed() {
    let mut mcp: serde_json::Value = serde_json::from_slice(MCP_FIXTURE).unwrap();
    mcp["artifact"]["tag"] = serde_json::json!("latest");
    let error = McpReleaseDescriptor::from_json(&serde_json::to_vec(&mcp).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.release.descriptor_invalid");

    let mut skill: serde_json::Value = serde_json::from_slice(SKILL_FIXTURE).unwrap();
    skill["runtime"] = serde_json::json!({"command": ["/bin/sh"]});
    let error =
        SkillReleaseDescriptor::from_json(&serde_json::to_vec(&skill).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.release.descriptor_invalid");

    let mut mcp: serde_json::Value = serde_json::from_slice(MCP_FIXTURE).unwrap();
    mcp["service"]["transport"] = serde_json::json!("stdio");
    let error = McpReleaseDescriptor::from_json(&serde_json::to_vec(&mcp).unwrap()).unwrap_err();
    assert_eq!(error.code, "use.release.descriptor_invalid");
}

#[test]
fn descriptors_reject_noncanonical_identity_and_set_values() {
    let mut mcp = McpReleaseDescriptor::from_json(MCP_FIXTURE).unwrap();
    mcp.provenance.source_repository = "https://token@example.com/a3s/private.git".to_string();
    assert_eq!(
        mcp.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut skill = SkillReleaseDescriptor::from_json(SKILL_FIXTURE).unwrap();
    skill.skill.required_capabilities.swap(0, 1);
    assert_eq!(
        skill.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut mcp = McpReleaseDescriptor::from_json(MCP_FIXTURE).unwrap();
    mcp.artifact.digest.make_ascii_uppercase();
    assert_eq!(
        mcp.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut mcp = McpReleaseDescriptor::from_json(MCP_FIXTURE).unwrap();
    let mut conflicting_dependency = mcp.dependencies[0].clone();
    conflicting_dependency.version = "2.0.0".to_string();
    mcp.dependencies.push(conflicting_dependency);
    mcp.dependencies.sort();
    assert_eq!(
        mcp.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut skill = SkillReleaseDescriptor::from_json(SKILL_FIXTURE).unwrap();
    skill.skill.entrypoint = "skills/example/./SKILL.md".to_string();
    assert_eq!(
        skill.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );

    let mut mcp = McpReleaseDescriptor::from_json(MCP_FIXTURE).unwrap();
    mcp.service.protocol_version = "2025-02-31".to_string();
    assert_eq!(
        mcp.canonical_bytes().unwrap_err().code,
        "use.release.descriptor_invalid"
    );
}

#[test]
fn decode_diagnostics_do_not_echo_descriptor_values() {
    let secret_marker = "do-not-echo-super-secret";
    let mut mcp: serde_json::Value = serde_json::from_slice(MCP_FIXTURE).unwrap();
    mcp["kind"] = serde_json::json!(secret_marker);

    let error = McpReleaseDescriptor::from_json(&serde_json::to_vec(&mcp).unwrap()).unwrap_err();

    assert_eq!(error.code, "use.release.descriptor_invalid");
    assert!(!error.message.contains(secret_marker));
}

#[test]
fn compatibility_and_dependency_resolution_fail_before_deployment() {
    let mcp = McpReleaseDescriptor::from_json(MCP_FIXTURE).unwrap();
    let mut resolution = ReleaseResolution {
        components: BTreeMap::from([
            ("a3s-runtime".to_string(), "0.2.0".to_string()),
            ("a3s-use".to_string(), "0.1.2".to_string()),
        ]),
        dependencies: mcp.dependencies.clone(),
    };
    mcp.verify_resolution(&resolution).unwrap();

    resolution.components.remove("a3s-runtime");
    assert_eq!(
        mcp.verify_resolution(&resolution).unwrap_err().code,
        "use.release.compatibility_missing"
    );
    resolution
        .components
        .insert("a3s-runtime".to_string(), "0.1.9".to_string());
    assert_eq!(
        mcp.verify_resolution(&resolution).unwrap_err().code,
        "use.release.incompatible"
    );

    resolution
        .components
        .insert("a3s-runtime".to_string(), "0.2.0".to_string());
    resolution.dependencies.clear();
    assert_eq!(
        mcp.verify_resolution(&resolution).unwrap_err().code,
        "use.release.dependency_missing"
    );

    resolution.dependencies = mcp.dependencies.clone();
    resolution.dependencies[0].version = "0.9.0".to_string();
    assert_eq!(
        mcp.verify_resolution(&resolution).unwrap_err().code,
        "use.release.dependency_mismatch"
    );

    resolution.dependencies = mcp.dependencies.clone();
    resolution.dependencies[0].descriptor_digest =
        "sha256:9999999999999999999999999999999999999999999999999999999999999999".to_string();
    assert_eq!(
        mcp.verify_resolution(&resolution).unwrap_err().code,
        "use.release.dependency_mismatch"
    );
}
