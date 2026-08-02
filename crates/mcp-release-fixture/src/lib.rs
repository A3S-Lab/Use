//! Cross-repository inputs for the headless MCP release conformance gate.

use a3s_use_core::{McpReleaseDescriptor, UseResult};
use serde::Serialize;

pub const MCP_PROTOCOL_VERSION: &str = "2025-06-18";
pub const MCP_RELEASE_TEMPLATE: &[u8] =
    include_bytes!("../../core/fixtures/releases/mcp-release-v1.json");

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RenderedMcpRelease {
    pub descriptor: McpReleaseDescriptor,
    pub descriptor_digest: String,
}

/// Bind the canonical MCP release fixture to one exact OCI image artifact.
pub fn render_mcp_release(
    artifact_digest: impl Into<String>,
    artifact_size_bytes: u64,
) -> UseResult<RenderedMcpRelease> {
    let mut descriptor = McpReleaseDescriptor::from_json(MCP_RELEASE_TEMPLATE)?;
    descriptor.artifact.digest = artifact_digest.into();
    descriptor.artifact.size_bytes = artifact_size_bytes;
    descriptor.validate()?;
    let descriptor_digest = descriptor.descriptor_digest()?;
    Ok(RenderedMcpRelease {
        descriptor,
        descriptor_digest,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use a3s_use_core::{ReleaseResolution, SkillReleaseDescriptor};

    use super::*;

    #[test]
    fn rendered_release_binds_the_exact_image_and_existing_skill_dependency() {
        let rendered = render_mcp_release(
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            4_096,
        )
        .unwrap();
        assert_eq!(
            rendered.descriptor.artifact.digest,
            "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
        assert_eq!(rendered.descriptor.artifact.size_bytes, 4_096);
        assert_eq!(
            rendered.descriptor.descriptor_digest().unwrap(),
            rendered.descriptor_digest
        );

        let skill = SkillReleaseDescriptor::from_json(include_bytes!(
            "../../core/fixtures/releases/skill-release-v1.json"
        ))
        .unwrap();
        let skill_descriptor_digest = skill.descriptor_digest().unwrap();
        rendered
            .descriptor
            .verify_resolution(&ReleaseResolution {
                components: BTreeMap::from([
                    ("a3s-runtime".to_string(), "0.2.0".to_string()),
                    ("a3s-use".to_string(), "0.1.2".to_string()),
                ]),
                dependencies: vec![a3s_use_core::ReleaseDependency {
                    kind: a3s_use_core::ReleaseKind::Skill,
                    name: skill.name,
                    version: skill.version,
                    descriptor_digest: skill_descriptor_digest,
                }],
            })
            .unwrap();
    }
}
