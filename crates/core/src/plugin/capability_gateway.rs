//! Path-free capability contracts for the lower-authority agent gateway.
//!
//! The package lifecycle owns the private materialization of a capability.
//! This module only describes the portable boundary that an arbitrary agent
//! may discover.  In particular, it deliberately carries no executable path,
//! package root, provider detail, bearer credential, or mutable operation
//! state.  An embedding host resolves the opaque references server-side while
//! retaining the exact package-generation lease for the lifetime of a call.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::de::Error as _;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::UseResult;

use super::validation::{strictly_sorted_unique, valid_segment, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, InstallationId,
    PluginSurfaceKind, PluginSurfaceRef,
};

/// Current portable description of one agent-visible capability.
pub const CAPABILITY_DESCRIPTOR_SCHEMA_V1: &str = "a3s.use.capability-descriptor.v1";
/// Current immutable index exchanged with a Capability Gateway host.
pub const CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1: &str = "a3s.use.capability-gateway-catalog.v1";
mod description_proof;
pub use description_proof::{CapabilityDescriptionProof, CAPABILITY_DESCRIPTION_PROOF_SCHEMA_V1};

const CAPABILITY_ERROR: &str = "use.plugin.capability_gateway_invalid";
const CAPABILITY_REF_DOMAIN: &[u8] = b"a3s.use.capability-ref.v1\0";
const MAX_CAPABILITY_TEXT_BYTES: usize = 4 * 1024;
const MAX_CAPABILITY_PROTOCOL_BYTES: usize = 128;
const MAX_CAPABILITY_DEPENDENCIES: usize = 64;
const MAX_CAPABILITY_DESCRIPTORS: usize = 1_024;
const MAX_CAPABILITY_SCHEMA_BYTES: usize = 64 * 1024;
const MAX_CAPABILITY_SCHEMA_DEPTH: usize = 16;
const MAX_CAPABILITY_SCHEMA_PROPERTIES: usize = 256;

/// An opaque server-resolved invocation identity.
///
/// The value is intentionally not a URL and does not encode a local path.
/// Hosts must map it to a private invocation binding rather than treating the
/// string as an instruction from the client.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct InvocationRef(String);

/// An opaque reference to verified package-owned content.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ArtifactRef(String);

/// An opaque reference to a host-owned endpoint binding.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct EndpointRef(String);

/// An opaque reference to one host-resolved MCP resource.
///
/// Resource URIs are deliberately not ordinary URLs. The Gateway resolves the
/// reference inside the host authority, so a consumer cannot turn discovery
/// metadata into a filesystem, network, or package-root lookup.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ResourceRef(String);

macro_rules! opaque_ref_impl {
    ($type:ty, $prefix:literal, $label:literal, $domain:literal) => {
        impl $type {
            /// Parse a reference received over the wire.
            pub fn parse(value: impl Into<String>) -> UseResult<Self> {
                let value = value.into();
                validate_opaque_ref(&value, $prefix, $label)?;
                Ok(Self(value))
            }

            /// Return the stable wire representation.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Derive a deterministic reference from verified host identity.
            ///
            /// `binding_digest` is the digest of the host-owned binding, not
            /// a value supplied by an agent.  The domain separator prevents
            /// invocation, artifact, and endpoint references from colliding.
            pub fn derive(
                package_id: &super::PluginPackageId,
                surface: &super::PluginSurfaceRef,
                generation: u64,
                binding_digest: &str,
            ) -> UseResult<Self> {
                let value = derive_opaque_ref(
                    $prefix,
                    $domain,
                    package_id,
                    surface,
                    generation,
                    binding_digest,
                )?;
                Ok(Self(value))
            }
        }

        impl<'de> Deserialize<'de> for $type {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(D::Error::custom)
            }
        }

        impl AsRef<str> for $type {
            fn as_ref(&self) -> &str {
                self.as_str()
            }
        }

        impl fmt::Display for $type {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(self.as_str())
            }
        }
    };
}

opaque_ref_impl!(
    InvocationRef,
    "invocation:v1:",
    "InvocationRef",
    "invocation"
);
opaque_ref_impl!(ArtifactRef, "artifact:v1:", "ArtifactRef", "artifact");
opaque_ref_impl!(EndpointRef, "endpoint:v1:", "EndpointRef", "endpoint");
opaque_ref_impl!(ResourceRef, "resource:v1:", "ResourceRef", "resource");

/// Hints exposed to an MCP-capable consumer for one Tool.
///
/// These are descriptive hints only.  Authorization and mutation policy stay
/// in the host and are never inferred from a client-provided hint.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityToolAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
    pub idempotent_hint: bool,
    pub open_world_hint: bool,
}

impl CapabilityToolAnnotations {
    pub const fn new(
        read_only_hint: bool,
        destructive_hint: bool,
        idempotent_hint: bool,
        open_world_hint: bool,
    ) -> Self {
        Self {
            read_only_hint,
            destructive_hint,
            idempotent_hint,
            open_world_hint,
        }
    }
}

/// Transport that can be represented by a lower-authority gateway descriptor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityMcpTransport {
    StreamableHttp,
}

/// Agent-visible part of a capability description.
///
/// Executable-only Tool Tasks intentionally have no variant here.  A host may
/// add one only after it has produced a schema-valid, non-pathful descriptor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "kebab-case",
    rename_all_fields = "camelCase"
)]
pub enum CapabilityDescriptorKind {
    Tool {
        name: String,
        input_schema: Value,
        output_schema: Value,
        annotations: CapabilityToolAnnotations,
    },
    McpServer {
        server_name: String,
        transport: CapabilityMcpTransport,
        protocol_version: String,
    },
    /// A standard MCP resource whose contents are resolved by the host.
    Resource {
        name: String,
        uri: ResourceRef,
        #[serde(skip_serializing_if = "Option::is_none")]
        mime_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        size: Option<u32>,
    },
    /// A standard MCP prompt whose messages are generated by the host.
    Prompt {
        name: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        arguments: Vec<CapabilityPromptArgument>,
    },
}

/// A bounded argument declaration for an MCP prompt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityPromptArgument {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
}

impl CapabilityPromptArgument {
    pub fn new(name: impl Into<String>, required: bool) -> Self {
        Self {
            name: name.into(),
            title: None,
            description: None,
            required,
        }
    }

    fn validate(&self) -> UseResult<()> {
        if !valid_tool_name(&self.name)
            || self
                .title
                .as_deref()
                .is_some_and(|value| !valid_capability_text(value, MAX_CAPABILITY_TEXT_BYTES))
            || self
                .description
                .as_deref()
                .is_some_and(|value| !valid_capability_text(value, MAX_CAPABILITY_TEXT_BYTES))
        {
            return Err(capability_error(
                "An MCP prompt argument has an invalid name or description.",
            ));
        }
        Ok(())
    }
}

/// Evidence binding a description to a verified publication.
///
/// The signature bytes remain in the Registry trust boundary.  The gateway
/// carries only their content digest, which lets a host reject stale or
/// substituted descriptions without disclosing signing material.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityPublicationEvidence {
    pub catalog_record_digest: String,
    pub signature_digest: String,
}

/// One path-free capability advertised to an agent, bound to its owning
/// package lifecycle generation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDescriptor {
    pub schema: String,
    pub package_id: super::PluginPackageId,
    pub surface: PluginSurfaceRef,
    pub generation: u64,
    pub package_digest: String,
    pub manifest_digest: String,
    pub title: String,
    pub description: String,
    pub invocation_ref: InvocationRef,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub artifact_ref: Option<ArtifactRef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint_ref: Option<EndpointRef>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<PluginSurfaceRef>,
    pub publication: CapabilityPublicationEvidence,
    #[serde(flatten)]
    pub capability: CapabilityDescriptorKind,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CapabilityDescriptorWire {
    schema: String,
    package_id: super::PluginPackageId,
    surface: PluginSurfaceRef,
    generation: u64,
    package_digest: String,
    manifest_digest: String,
    title: String,
    description: String,
    invocation_ref: InvocationRef,
    #[serde(default)]
    artifact_ref: Option<ArtifactRef>,
    #[serde(default)]
    endpoint_ref: Option<EndpointRef>,
    #[serde(default)]
    dependencies: Vec<PluginSurfaceRef>,
    publication: CapabilityPublicationEvidence,
    kind: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    input_schema: Option<Value>,
    #[serde(default)]
    output_schema: Option<Value>,
    #[serde(default)]
    annotations: Option<CapabilityToolAnnotations>,
    #[serde(default)]
    server_name: Option<String>,
    #[serde(default)]
    transport: Option<CapabilityMcpTransport>,
    #[serde(default)]
    protocol_version: Option<String>,
    #[serde(default)]
    uri: Option<ResourceRef>,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default)]
    size: Option<u32>,
    #[serde(default)]
    arguments: Option<Vec<CapabilityPromptArgument>>,
}

impl<'de> Deserialize<'de> for CapabilityDescriptor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        decode_capability_descriptor(value).map_err(D::Error::custom)
    }
}

fn decode_capability_descriptor(value: Value) -> Result<CapabilityDescriptor, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "A capability descriptor must be a JSON object.".to_owned())?;
    const ALLOWED_FIELDS: &[&str] = &[
        "schema",
        "packageId",
        "surface",
        "generation",
        "packageDigest",
        "manifestDigest",
        "title",
        "description",
        "invocationRef",
        "artifactRef",
        "endpointRef",
        "dependencies",
        "publication",
        "kind",
        "name",
        "inputSchema",
        "outputSchema",
        "annotations",
        "serverName",
        "transport",
        "protocolVersion",
        "uri",
        "mimeType",
        "size",
        "arguments",
    ];
    if let Some(field) = object
        .keys()
        .find(|field| !ALLOWED_FIELDS.contains(&field.as_str()))
    {
        return Err(format!("Unknown capability descriptor field `{field}`."));
    }

    let wire: CapabilityDescriptorWire = serde_json::from_value(value)
        .map_err(|error| format!("Invalid capability descriptor: {error}"))?;
    let capability = match wire.kind.as_str() {
        "tool" => {
            if wire.server_name.is_some()
                || wire.transport.is_some()
                || wire.protocol_version.is_some()
                || wire.uri.is_some()
                || wire.mime_type.is_some()
                || wire.size.is_some()
                || wire.arguments.is_some()
            {
                return Err(
                    "A Tool descriptor contains fields for another capability kind.".to_owned(),
                );
            }
            CapabilityDescriptorKind::Tool {
                name: wire
                    .name
                    .ok_or_else(|| "A Tool descriptor requires `name`.".to_owned())?,
                input_schema: wire
                    .input_schema
                    .ok_or_else(|| "A Tool descriptor requires `inputSchema`.".to_owned())?,
                output_schema: wire
                    .output_schema
                    .ok_or_else(|| "A Tool descriptor requires `outputSchema`.".to_owned())?,
                annotations: wire
                    .annotations
                    .ok_or_else(|| "A Tool descriptor requires `annotations`.".to_owned())?,
            }
        }
        "mcp-server" => {
            if wire.name.is_some()
                || wire.input_schema.is_some()
                || wire.output_schema.is_some()
                || wire.annotations.is_some()
                || wire.uri.is_some()
                || wire.mime_type.is_some()
                || wire.size.is_some()
                || wire.arguments.is_some()
            {
                return Err(
                    "An MCP Server descriptor contains fields for another capability kind."
                        .to_owned(),
                );
            }
            CapabilityDescriptorKind::McpServer {
                server_name: wire
                    .server_name
                    .ok_or_else(|| "An MCP Server descriptor requires `serverName`.".to_owned())?,
                transport: wire
                    .transport
                    .ok_or_else(|| "An MCP Server descriptor requires `transport`.".to_owned())?,
                protocol_version: wire.protocol_version.ok_or_else(|| {
                    "An MCP Server descriptor requires `protocolVersion`.".to_owned()
                })?,
            }
        }
        "resource" => {
            if wire.input_schema.is_some()
                || wire.output_schema.is_some()
                || wire.annotations.is_some()
                || wire.server_name.is_some()
                || wire.transport.is_some()
                || wire.protocol_version.is_some()
                || wire.arguments.is_some()
            {
                return Err(
                    "A Resource descriptor contains fields for another capability kind.".to_owned(),
                );
            }
            CapabilityDescriptorKind::Resource {
                name: wire
                    .name
                    .ok_or_else(|| "A Resource descriptor requires `name`.".to_owned())?,
                uri: wire
                    .uri
                    .ok_or_else(|| "A Resource descriptor requires `uri`.".to_owned())?,
                mime_type: wire.mime_type,
                size: wire.size,
            }
        }
        "prompt" => {
            if wire.input_schema.is_some()
                || wire.output_schema.is_some()
                || wire.annotations.is_some()
                || wire.server_name.is_some()
                || wire.transport.is_some()
                || wire.protocol_version.is_some()
                || wire.uri.is_some()
                || wire.mime_type.is_some()
                || wire.size.is_some()
            {
                return Err(
                    "A Prompt descriptor contains fields for another capability kind.".to_owned(),
                );
            }
            CapabilityDescriptorKind::Prompt {
                name: wire
                    .name
                    .ok_or_else(|| "A Prompt descriptor requires `name`.".to_owned())?,
                arguments: wire.arguments.unwrap_or_default(),
            }
        }
        other => return Err(format!("Unsupported capability descriptor kind `{other}`.")),
    };

    Ok(CapabilityDescriptor {
        schema: wire.schema,
        package_id: wire.package_id,
        surface: wire.surface,
        generation: wire.generation,
        package_digest: wire.package_digest,
        manifest_digest: wire.manifest_digest,
        title: wire.title,
        description: wire.description,
        invocation_ref: wire.invocation_ref,
        artifact_ref: wire.artifact_ref,
        endpoint_ref: wire.endpoint_ref,
        dependencies: wire.dependencies,
        publication: wire.publication,
        capability,
    })
}

/// Immutable capability index for one installation and one capability
/// publication generation.
///
/// `generation` identifies the publication as a whole. Each descriptor keeps
/// the lifecycle generation of its owning package, and those values may differ
/// when one publication contains several independently upgraded packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CapabilityGatewayCatalog {
    pub schema: String,
    pub installation: InstallationId,
    pub generation: u64,
    pub revision: String,
    pub descriptors: Vec<CapabilityDescriptor>,
}

impl CapabilityDescriptor {
    /// Decode and validate one bounded descriptor document.
    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "Capability descriptor",
            CAPABILITY_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_DESCRIPTOR_SCHEMA_V1
            || !super::PluginPackageId::is_valid(self.package_id.as_str())
            || validate_surface(&self.surface).is_err()
            || self.generation == 0
            || !valid_sha256(&self.package_digest)
            || !valid_sha256(&self.manifest_digest)
            || !valid_capability_text(&self.title, MAX_CAPABILITY_TEXT_BYTES)
            || !valid_capability_text(&self.description, MAX_CAPABILITY_TEXT_BYTES)
            || self.dependencies.len() > MAX_CAPABILITY_DEPENDENCIES
            || !strictly_sorted_unique(&self.dependencies)
            || self
                .dependencies
                .iter()
                .any(|dependency| validate_surface(dependency).is_err())
        {
            return Err(capability_error(
                "The capability descriptor identity, text, digest, or dependency set is invalid.",
            ));
        }

        self.publication.validate()?;
        validate_opaque_ref(
            self.invocation_ref.as_str(),
            "invocation:v1:",
            "InvocationRef",
        )?;
        if let Some(reference) = &self.artifact_ref {
            validate_opaque_ref(reference.as_str(), "artifact:v1:", "ArtifactRef")?;
        }
        if let Some(reference) = &self.endpoint_ref {
            validate_opaque_ref(reference.as_str(), "endpoint:v1:", "EndpointRef")?;
        }
        if self
            .dependencies
            .iter()
            .any(|dependency| dependency == &self.surface)
        {
            return Err(capability_error(
                "A capability descriptor cannot depend on its own surface.",
            ));
        }

        match &self.capability {
            CapabilityDescriptorKind::Tool {
                name,
                input_schema,
                output_schema,
                annotations: _,
            } => {
                if self.surface.kind != PluginSurfaceKind::Tool
                    || !valid_tool_name(name)
                    || validate_agent_schema(input_schema, true).is_err()
                    || validate_agent_schema(output_schema, true).is_err()
                {
                    return Err(capability_error(
                        "A Tool descriptor must bind a schema-valid Tool surface.",
                    ));
                }
            }
            CapabilityDescriptorKind::McpServer {
                server_name,
                transport: CapabilityMcpTransport::StreamableHttp,
                protocol_version,
            } => {
                if self.surface.kind != PluginSurfaceKind::Mcp
                    || !valid_tool_name(server_name)
                    || protocol_version.is_empty()
                    || protocol_version.len() > MAX_CAPABILITY_PROTOCOL_BYTES
                    || protocol_version.chars().any(char::is_control)
                    || self.endpoint_ref.is_none()
                {
                    return Err(capability_error(
                        "An MCP Server descriptor must bind a streamable HTTP endpoint.",
                    ));
                }
            }
            CapabilityDescriptorKind::Resource {
                name,
                uri,
                mime_type,
                size: _,
            } => {
                if !valid_tool_name(name)
                    || validate_opaque_ref(uri.as_str(), "resource:v1:", "ResourceRef").is_err()
                    || mime_type.as_deref().is_some_and(|value| {
                        value.is_empty()
                            || value.len() > MAX_CAPABILITY_PROTOCOL_BYTES
                            || value.chars().any(char::is_control)
                    })
                {
                    return Err(capability_error(
                        "A Resource descriptor must bind a valid opaque resource reference.",
                    ));
                }
            }
            CapabilityDescriptorKind::Prompt { name, arguments } => {
                if !valid_tool_name(name)
                    || arguments.len() > MAX_CAPABILITY_DEPENDENCIES
                    || !strictly_sorted_unique(
                        &arguments
                            .iter()
                            .map(|argument| argument.name.as_str())
                            .collect::<Vec<_>>(),
                    )
                {
                    return Err(capability_error(
                        "A Prompt descriptor has an invalid or unordered argument set.",
                    ));
                }
                for argument in arguments {
                    argument.validate()?;
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "capability descriptor", CAPABILITY_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn tool_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Tool { name, .. } => Some(name),
            CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. } => None,
        }
    }

    pub fn mcp_server_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. } => None,
            CapabilityDescriptorKind::McpServer { server_name, .. } => Some(server_name),
        }
    }

    pub fn resource_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Resource { name, .. } => Some(name),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Prompt { .. } => None,
        }
    }

    pub fn resource_uri(&self) -> Option<&ResourceRef> {
        match &self.capability {
            CapabilityDescriptorKind::Resource { uri, .. } => Some(uri),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Prompt { .. } => None,
        }
    }

    pub fn prompt_name(&self) -> Option<&str> {
        match &self.capability {
            CapabilityDescriptorKind::Prompt { name, .. } => Some(name),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. } => None,
        }
    }

    pub fn prompt_arguments(&self) -> Option<&[CapabilityPromptArgument]> {
        match &self.capability {
            CapabilityDescriptorKind::Prompt { arguments, .. } => Some(arguments),
            CapabilityDescriptorKind::Tool { .. }
            | CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. } => None,
        }
    }

    pub fn is_agent_tool(&self) -> bool {
        matches!(self.capability, CapabilityDescriptorKind::Tool { .. })
    }

    pub fn is_resource(&self) -> bool {
        matches!(self.capability, CapabilityDescriptorKind::Resource { .. })
    }

    pub fn is_prompt(&self) -> bool {
        matches!(self.capability, CapabilityDescriptorKind::Prompt { .. })
    }
}

impl CapabilityPublicationEvidence {
    pub fn validate(&self) -> UseResult<()> {
        if !valid_sha256(&self.catalog_record_digest) || !valid_sha256(&self.signature_digest) {
            return Err(capability_error(
                "Capability publication evidence must bind a catalog record and signature digest.",
            ));
        }
        Ok(())
    }
}

impl CapabilityGatewayCatalog {
    /// Build a canonical immutable catalog. Input descriptors are sorted by
    /// package, surface, and package lifecycle generation before the revision
    /// is allocated. `generation` is the publication generation, not a package
    /// lifecycle generation.
    pub fn new(
        installation: InstallationId,
        generation: u64,
        mut descriptors: Vec<CapabilityDescriptor>,
    ) -> UseResult<Self> {
        installation.validate()?;
        descriptors.sort_by(descriptor_order);
        let revision = catalog_revision(&installation, generation, &descriptors)?;
        let catalog = Self {
            schema: CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1.to_owned(),
            installation,
            generation,
            revision,
            descriptors,
        };
        catalog.validate()?;
        Ok(catalog)
    }

    /// Build a catalog only from descriptions that have crossed the host's
    /// signed-publication verification boundary.
    pub fn from_verified_descriptions(
        installation: InstallationId,
        generation: u64,
        proofs: Vec<CapabilityDescriptionProof>,
    ) -> UseResult<Self> {
        let descriptors = proofs
            .into_iter()
            .map(|proof| {
                proof.validate()?;
                Ok(proof.into_descriptor())
            })
            .collect::<UseResult<Vec<_>>>()?;
        Self::new(installation, generation, descriptors)
    }

    pub fn from_json(input: &[u8]) -> UseResult<Self> {
        parse_contract(
            input,
            "Capability Gateway catalog",
            CAPABILITY_ERROR,
            Self::validate,
        )
    }

    pub fn validate(&self) -> UseResult<()> {
        if self.schema != CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1
            || self.installation.validate().is_err()
            || self.descriptors.len() > MAX_CAPABILITY_DESCRIPTORS
            || !valid_sha256(&self.revision)
            || (self.generation == 0 && !self.descriptors.is_empty())
        {
            return Err(capability_error(
                "The Capability Gateway catalog identity or bounds are invalid.",
            ));
        }

        let mut identities = BTreeSet::new();
        let mut tool_names = BTreeSet::new();
        let mut mcp_server_names = BTreeSet::new();
        let mut resource_uris = BTreeSet::new();
        let mut prompt_names = BTreeSet::new();
        let mut surface_generations = BTreeMap::new();
        let mut previous = None;
        for descriptor in &self.descriptors {
            descriptor.validate()?;
            let identity = descriptor_identity_key(descriptor);
            if previous
                .as_ref()
                .is_some_and(|value| *value >= descriptor_order_key(descriptor))
                || !identities.insert(identity)
            {
                return Err(capability_error(
                    "Capability descriptors must be sorted and unique within one publication.",
                ));
            }
            let surface_key = descriptor_surface_key(descriptor);
            if surface_generations
                .insert(surface_key, descriptor.generation)
                .is_some_and(|generation| generation != descriptor.generation)
            {
                return Err(capability_error(
                    "A capability surface cannot publish multiple lifecycle generations in one catalog.",
                ));
            }
            if let Some(name) = descriptor.tool_name() {
                if !tool_names.insert(name) {
                    return Err(capability_error(
                        "Capability Gateway Tool names must be unique within one catalog.",
                    ));
                }
            }
            if let Some(name) = descriptor.mcp_server_name() {
                if !mcp_server_names.insert(name) {
                    return Err(capability_error(
                        "Capability Gateway MCP server names must be unique within one catalog.",
                    ));
                }
            }
            if let Some(uri) = descriptor.resource_uri() {
                if !resource_uris.insert(uri.as_str()) {
                    return Err(capability_error(
                        "Capability Gateway resource URIs must be unique within one catalog.",
                    ));
                }
            }
            if let Some(name) = descriptor.prompt_name() {
                if !prompt_names.insert(name) {
                    return Err(capability_error(
                        "Capability Gateway prompt names must be unique within one catalog.",
                    ));
                }
            }
            previous = Some(descriptor_order_key(descriptor));
        }

        let expected_revision =
            catalog_revision(&self.installation, self.generation, &self.descriptors)?;
        if self.revision != expected_revision {
            return Err(capability_error(
                "The Capability Gateway catalog revision does not match its immutable descriptors.",
            ));
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> UseResult<Vec<u8>> {
        self.validate()?;
        canonical_json(self, "Capability Gateway catalog", CAPABILITY_ERROR)
    }

    pub fn descriptor_digest(&self) -> UseResult<String> {
        Ok(canonical_digest(&self.canonical_bytes()?))
    }

    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn revision(&self) -> &str {
        &self.revision
    }

    pub fn descriptors(&self) -> &[CapabilityDescriptor] {
        &self.descriptors
    }

    pub fn find_tool(&self, name: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.tool_name() == Some(name))
    }

    pub fn find_resource(&self, uri: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors.iter().find(|descriptor| {
            descriptor
                .resource_uri()
                .is_some_and(|value| value.as_str() == uri)
        })
    }

    pub fn find_prompt(&self, name: &str) -> Option<&CapabilityDescriptor> {
        self.descriptors
            .iter()
            .find(|descriptor| descriptor.prompt_name() == Some(name))
    }
}

fn descriptor_order(
    left: &CapabilityDescriptor,
    right: &CapabilityDescriptor,
) -> std::cmp::Ordering {
    descriptor_order_key(left).cmp(&descriptor_order_key(right))
}

fn descriptor_order_key(
    descriptor: &CapabilityDescriptor,
) -> (String, PluginSurfaceKind, String, u64, String) {
    (
        descriptor.package_id.to_string(),
        descriptor.surface.kind,
        descriptor.surface.id.clone(),
        descriptor.generation,
        descriptor_capability_key(descriptor),
    )
}

fn descriptor_identity_key(
    descriptor: &CapabilityDescriptor,
) -> (String, PluginSurfaceKind, String, String) {
    (
        descriptor.package_id.to_string(),
        descriptor.surface.kind,
        descriptor.surface.id.clone(),
        descriptor_capability_key(descriptor),
    )
}

fn descriptor_surface_key(
    descriptor: &CapabilityDescriptor,
) -> (String, PluginSurfaceKind, String) {
    (
        descriptor.package_id.to_string(),
        descriptor.surface.kind,
        descriptor.surface.id.clone(),
    )
}

fn descriptor_capability_key(descriptor: &CapabilityDescriptor) -> String {
    match &descriptor.capability {
        CapabilityDescriptorKind::Tool { name, .. } => format!("tool:{name}"),
        CapabilityDescriptorKind::McpServer { server_name, .. } => {
            format!("mcp-server:{server_name}")
        }
        CapabilityDescriptorKind::Resource { uri, .. } => format!("resource:{}", uri.as_str()),
        CapabilityDescriptorKind::Prompt { name, .. } => format!("prompt:{name}"),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CatalogRevisionMaterial<'a> {
    schema: &'static str,
    installation: &'a InstallationId,
    generation: u64,
    descriptors: &'a [CapabilityDescriptor],
}

fn catalog_revision(
    installation: &InstallationId,
    generation: u64,
    descriptors: &[CapabilityDescriptor],
) -> UseResult<String> {
    let material = CatalogRevisionMaterial {
        schema: CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1,
        installation,
        generation,
        descriptors,
    };
    Ok(canonical_digest(&canonical_json(
        &material,
        "Capability Gateway catalog revision",
        CAPABILITY_ERROR,
    )?))
}

fn validate_surface(surface: &PluginSurfaceRef) -> UseResult<()> {
    if !valid_segment(&surface.id) {
        return Err(capability_error("A capability surface ID is invalid."));
    }
    Ok(())
}

fn valid_tool_name(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && matches!(value.as_bytes().first(), Some(b'a'..=b'z'))
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn valid_capability_text(value: &str, max_bytes: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_bytes
        && value.trim() == value
        && !value.chars().any(char::is_control)
}

fn validate_opaque_ref(value: &str, prefix: &str, label: &str) -> UseResult<()> {
    let digest = value
        .strip_prefix(prefix)
        .and_then(|value| value.strip_prefix("sha256:"));
    if digest.is_none_or(|digest| {
        digest.len() != 64
            || !digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    }) {
        return Err(capability_error(format!(
            "The {label} value is not a valid opaque reference."
        )));
    }
    Ok(())
}

fn derive_opaque_ref(
    prefix: &str,
    reference_domain: &str,
    package_id: &super::PluginPackageId,
    surface: &PluginSurfaceRef,
    generation: u64,
    binding_digest: &str,
) -> UseResult<String> {
    if !super::PluginPackageId::is_valid(package_id.as_str()) {
        return Err(capability_error("The package identity is invalid."));
    }
    validate_surface(surface)?;
    if generation == 0 || !valid_sha256(binding_digest) {
        return Err(capability_error(
            "Opaque references require a positive generation and a binding digest.",
        ));
    }
    let mut digest = Sha256::new();
    digest.update(CAPABILITY_REF_DOMAIN);
    for value in [
        reference_domain,
        package_id.as_str(),
        surface_kind_name(surface.kind),
        surface.id.as_str(),
        &generation.to_string(),
        binding_digest,
    ] {
        digest.update(value.as_bytes());
        digest.update([0]);
    }
    Ok(format!("{prefix}sha256:{:x}", digest.finalize()))
}

fn surface_kind_name(kind: PluginSurfaceKind) -> &'static str {
    match kind {
        PluginSurfaceKind::Flow => "flow",
        PluginSurfaceKind::Mcp => "mcp",
        PluginSurfaceKind::Okf => "okf",
        PluginSurfaceKind::Skill => "skill",
        PluginSurfaceKind::Tool => "tool",
        PluginSurfaceKind::Ui => "ui",
    }
}

fn validate_agent_schema(schema: &Value, require_object: bool) -> UseResult<()> {
    if !schema.is_object() {
        return Err(capability_error(
            "Agent input and output schemas must be JSON objects.",
        ));
    }
    let encoded = serde_json::to_vec(schema).map_err(|error| {
        capability_error(format!("Failed to encode an agent JSON schema: {error}"))
    })?;
    if encoded.len() > MAX_CAPABILITY_SCHEMA_BYTES {
        return Err(capability_error(
            "An agent JSON schema exceeds its size bound.",
        ));
    }
    validate_schema_value(schema, 0)?;
    if require_object {
        let Some(object) = schema.as_object() else {
            return Err(capability_error(
                "Agent input and output schemas must be JSON objects.",
            ));
        };
        if object.get("type") != Some(&Value::String("object".to_owned())) {
            return Err(capability_error(
                "Agent Tool schemas must declare a top-level object type.",
            ));
        }
        if object.get("additionalProperties") != Some(&Value::Bool(false)) {
            return Err(capability_error(
                "Agent Tool schemas must close top-level additional properties.",
            ));
        }
    }
    Ok(())
}

fn validate_schema_value(value: &Value, depth: usize) -> UseResult<()> {
    if depth > MAX_CAPABILITY_SCHEMA_DEPTH {
        return Err(capability_error(
            "An agent JSON schema is nested too deeply.",
        ));
    }
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) => Ok(()),
        Value::String(text) => {
            if text.len() > MAX_CAPABILITY_TEXT_BYTES || text.chars().any(char::is_control) {
                Err(capability_error(
                    "An agent JSON schema contains unbounded text.",
                ))
            } else {
                Ok(())
            }
        }
        Value::Array(values) => {
            if values.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                return Err(capability_error("An agent JSON schema array is too large."));
            }
            for value in values {
                validate_schema_value(value, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(object) => {
            if object.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                return Err(capability_error(
                    "An agent JSON schema object is too large.",
                ));
            }
            for (key, value) in object {
                if key.is_empty()
                    || key.len() > 128
                    || key.chars().any(char::is_control)
                    || ((matches!(key.as_str(), "$ref" | "$dynamicRef" | "$recursiveRef"))
                        && !value
                            .as_str()
                            .is_some_and(|reference| reference.starts_with('#')))
                    || (key == "$id"
                        && !value
                            .as_str()
                            .is_some_and(|identifier| identifier.starts_with('#')))
                {
                    return Err(capability_error(
                        "An agent JSON schema contains an unsafe property or external reference.",
                    ));
                }
                validate_schema_value(value, depth + 1)?;
            }
            if let Some(properties_value) = object.get("properties") {
                let properties = properties_value.as_object().ok_or_else(|| {
                    capability_error("An agent JSON schema properties value must be an object.")
                })?;
                if properties.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                    return Err(capability_error(
                        "An agent JSON schema properties object is too large.",
                    ));
                }
                if let Some(required) = object.get("required") {
                    let required = required.as_array().ok_or_else(|| {
                        capability_error("An agent JSON schema required value must be an array.")
                    })?;
                    if required.len() > MAX_CAPABILITY_SCHEMA_PROPERTIES {
                        return Err(capability_error(
                            "An agent JSON schema required set is too large.",
                        ));
                    }
                    let names = required
                        .iter()
                        .map(|value| {
                            value.as_str().ok_or_else(|| {
                                capability_error(
                                    "An agent JSON schema required name is not a string.",
                                )
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    if !strictly_sorted_unique(&names)
                        || names.iter().any(|name| !properties.contains_key(*name))
                    {
                        return Err(capability_error(
                        "An agent JSON schema required set must be sorted and present in properties.",
                    ));
                    }
                }
            } else if object.contains_key("required") {
                return Err(capability_error(
                    "An agent JSON schema required set needs a properties object.",
                ));
            }
            Ok(())
        }
    }
}

fn capability_error(message: impl Into<String>) -> crate::UseError {
    contract_error(CAPABILITY_ERROR, message)
}

#[cfg(test)]
#[path = "capability_gateway_tests.rs"]
mod tests;
