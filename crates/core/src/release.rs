//! Immutable MCP, Skill, and Tool release descriptors shared with A3S Cloud.
//!
//! Descriptors are versioned machine-owned JSON. Their identity is the
//! SHA-256 of OLPC canonical JSON, so whitespace and object-key order never
//! change a release identity. Arrays representing sets must be sorted.

use std::collections::BTreeMap;

use olpc_cjson::CanonicalFormatter;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::UseResult;

use self::validation::{descriptor_error, validate_common, verify_resolution};

mod validation;

pub const MCP_RELEASE_SCHEMA: &str = "a3s.use.mcp-release.v1";
pub const SKILL_RELEASE_SCHEMA: &str = "a3s.use.skill-release.v1";
pub const TOOL_RELEASE_SCHEMA: &str = "a3s.use.tool-release.v1";
pub const MAX_RELEASE_DESCRIPTOR_BYTES: usize = 256 * 1024;

const OCI_IMAGE_MANIFEST: &str = "application/vnd.oci.image.manifest.v1+json";
const OCI_IMAGE_INDEX: &str = "application/vnd.oci.image.index.v1+json";
const SKILL_BUNDLE: &str = "application/vnd.a3s.skill.bundle.v1+tar+gzip";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReleaseKind {
    Mcp,
    Skill,
    Tool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseProvenance {
    pub source_repository: String,
    pub commit_sha: String,
    pub manifest_digest: String,
    pub builder_id: String,
    pub build_operation_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseArtifact {
    pub media_type: String,
    pub digest: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseCompatibility {
    pub component: String,
    pub version_requirement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ReleaseDependency {
    pub kind: ReleaseKind,
    pub name: String,
    pub version: String,
    pub descriptor_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HttpHealthContract {
    pub path: String,
    pub interval_ms: u64,
    pub timeout_ms: u64,
    pub success_threshold: u32,
    pub failure_threshold: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum McpServiceTransport {
    StreamableHttp,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpServiceContract {
    pub transport: McpServiceTransport,
    pub protocol_version: String,
    pub port_name: String,
    pub port: u16,
    pub endpoint_path: String,
    pub health: HttpHealthContract,
    pub startup_timeout_ms: u64,
    pub shutdown_grace_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct McpReleaseDescriptor {
    pub schema: String,
    pub kind: ReleaseKind,
    pub name: String,
    pub version: String,
    pub provenance: ReleaseProvenance,
    pub artifact: ReleaseArtifact,
    pub compatibility: Vec<ReleaseCompatibility>,
    pub dependencies: Vec<ReleaseDependency>,
    pub service: McpServiceContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SkillBindingTarget {
    AgentInput,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillBindingContract {
    pub target: SkillBindingTarget,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillContentContract {
    pub entrypoint: String,
    pub entrypoint_digest: String,
    pub required_capabilities: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SkillReleaseDescriptor {
    pub schema: String,
    pub kind: ReleaseKind,
    pub name: String,
    pub version: String,
    pub provenance: ReleaseProvenance,
    pub artifact: ReleaseArtifact,
    pub compatibility: Vec<ReleaseCompatibility>,
    pub dependencies: Vec<ReleaseDependency>,
    pub skill: SkillContentContract,
    pub binding: SkillBindingContract,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolTaskInterface {
    Cli,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolServiceInterface {
    Http,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToolServiceNetwork {
    Private,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "class",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToolWorkloadContract {
    Task {
        interface: ToolTaskInterface,
        entrypoint: Vec<String>,
        interactive: bool,
        timeout_ms: u64,
        max_stdout_bytes: u64,
        max_stderr_bytes: u64,
        success_exit_codes: Vec<u8>,
    },
    Service {
        interface: ToolServiceInterface,
        network: ToolServiceNetwork,
        port_name: String,
        port: u16,
        base_path: String,
        health: HttpHealthContract,
        startup_timeout_ms: u64,
        shutdown_grace_ms: u64,
        #[serde(skip_serializing_if = "Option::is_none")]
        api_contract_digest: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolReleaseDescriptor {
    pub schema: String,
    pub kind: ReleaseKind,
    pub name: String,
    pub version: String,
    pub provenance: ReleaseProvenance,
    pub artifact: ReleaseArtifact,
    pub compatibility: Vec<ReleaseCompatibility>,
    pub dependencies: Vec<ReleaseDependency>,
    pub workload: ToolWorkloadContract,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseResolution {
    pub components: BTreeMap<String, String>,
    pub dependencies: Vec<ReleaseDependency>,
}

impl McpReleaseDescriptor {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_descriptor(input, "MCP", Self::validate)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != MCP_RELEASE_SCHEMA || self.kind != ReleaseKind::Mcp {
            return Err(descriptor_error(
                "The MCP release schema or kind is not supported.",
            ));
        }
        reject_tool_dependencies_in_legacy_v1(&self.dependencies)?;
        validate_common(
            &self.name,
            &self.version,
            &self.provenance,
            &self.artifact,
            &self.compatibility,
            &self.dependencies,
        )?;
        if !matches!(
            self.artifact.media_type.as_str(),
            OCI_IMAGE_MANIFEST | OCI_IMAGE_INDEX
        ) {
            return Err(descriptor_error(
                "An MCP release must reference a digest-pinned OCI image manifest or index.",
            ));
        }
        self.service.validate()
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(digest(self.canonical_bytes()?))
    }

    pub fn verify_resolution(&self, resolution: &ReleaseResolution) -> UseResult<()> {
        self.validate()?;
        verify_resolution(&self.compatibility, &self.dependencies, resolution)
    }
}

impl SkillReleaseDescriptor {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_descriptor(input, "Skill", Self::validate)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != SKILL_RELEASE_SCHEMA || self.kind != ReleaseKind::Skill {
            return Err(descriptor_error(
                "The Skill release schema or kind is not supported.",
            ));
        }
        reject_tool_dependencies_in_legacy_v1(&self.dependencies)?;
        validate_common(
            &self.name,
            &self.version,
            &self.provenance,
            &self.artifact,
            &self.compatibility,
            &self.dependencies,
        )?;
        if self.artifact.media_type != SKILL_BUNDLE {
            return Err(descriptor_error(
                "A Skill release must reference an A3S Skill bundle.",
            ));
        }
        self.skill.validate()
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(digest(self.canonical_bytes()?))
    }

    pub fn verify_resolution(&self, resolution: &ReleaseResolution) -> UseResult<()> {
        self.validate()?;
        verify_resolution(&self.compatibility, &self.dependencies, resolution)
    }
}

impl ToolReleaseDescriptor {
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_descriptor(input, "Tool", Self::validate)
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != TOOL_RELEASE_SCHEMA || self.kind != ReleaseKind::Tool {
            return Err(descriptor_error(
                "The Tool release schema or kind is not supported.",
            ));
        }
        validate_common(
            &self.name,
            &self.version,
            &self.provenance,
            &self.artifact,
            &self.compatibility,
            &self.dependencies,
        )?;
        if !matches!(
            self.artifact.media_type.as_str(),
            OCI_IMAGE_MANIFEST | OCI_IMAGE_INDEX
        ) {
            return Err(descriptor_error(
                "A Tool release must reference a digest-pinned OCI image manifest or index.",
            ));
        }
        self.workload.validate()
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(digest(self.canonical_bytes()?))
    }

    pub fn verify_resolution(&self, resolution: &ReleaseResolution) -> UseResult<()> {
        self.validate()?;
        verify_resolution(&self.compatibility, &self.dependencies, resolution)
    }
}

fn reject_tool_dependencies_in_legacy_v1(dependencies: &[ReleaseDependency]) -> UseResult<()> {
    if dependencies
        .iter()
        .any(|dependency| dependency.kind == ReleaseKind::Tool)
    {
        return Err(descriptor_error(
            "MCP and Skill release v1 descriptors cannot depend on Tool releases.",
        ));
    }
    Ok(())
}

fn parse_descriptor<T>(input: &[u8], label: &str, validate: fn(&T) -> UseResult<()>) -> UseResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    if input.is_empty() || input.len() > MAX_RELEASE_DESCRIPTOR_BYTES {
        return Err(descriptor_error(format!(
            "The {label} release descriptor exceeds its input bounds."
        )));
    }
    let descriptor = serde_json::from_slice(input).map_err(|error| {
        descriptor_error(format!(
            "Failed to decode the {label} release descriptor at line {}, column {}.",
            error.line(),
            error.column()
        ))
    })?;
    validate(&descriptor)?;
    Ok(descriptor)
}

fn canonical_json<T: Serialize>(value: &T) -> UseResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut bytes, CanonicalFormatter::new());
    value.serialize(&mut serializer).map_err(|error| {
        descriptor_error(format!("Failed to encode canonical release JSON: {error}"))
    })?;
    if bytes.len() > MAX_RELEASE_DESCRIPTOR_BYTES {
        return Err(descriptor_error(
            "The canonical release descriptor exceeds its size bound.",
        ));
    }
    Ok(bytes)
}

fn digest(bytes: Vec<u8>) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}
