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

use super::capability_consumer::{
    CapabilityConsumerExtension, CapabilityConsumerNegotiation, MAX_CAPABILITY_CONSUMER_EXTENSIONS,
};
use super::validation::{strictly_sorted_unique, valid_segment, valid_sha256};
use super::{
    canonical_digest, canonical_json, contract_error, parse_contract, InstallationId,
    PluginSurfaceKind, PluginSurfaceRef,
};

/// Current portable description of one agent-visible capability.
pub const CAPABILITY_DESCRIPTOR_SCHEMA_V1: &str = "a3s.use.capability-descriptor.v1";
/// Current immutable index exchanged with a Capability Gateway host.
pub const CAPABILITY_GATEWAY_CATALOG_SCHEMA_V1: &str = "a3s.use.capability-gateway-catalog.v1";
/// Domain-separated digest used to bind a Tool's JSON contract across the
/// signed capability description and the Runtime payload.
pub const CAPABILITY_SCHEMA_DIGEST_SCHEMA_V1: &str = "a3s.use.capability-schema-digest.v1";
mod description_proof;
mod description_signature;
pub use description_proof::{CapabilityDescriptionProof, CAPABILITY_DESCRIPTION_PROOF_SCHEMA_V1};
pub use description_signature::{
    CapabilityDescriptionSignatureAlgorithm, CapabilityDescriptionSignaturePayload,
    SignedCapabilityDescription, CAPABILITY_DESCRIPTION_SIGNATURE_ALGORITHM_ED25519,
    CAPABILITY_DESCRIPTION_SIGNATURE_SCHEMA_V1,
};

const CAPABILITY_ERROR: &str = "use.plugin.capability_gateway_invalid";
const CAPABILITY_REF_DOMAIN: &[u8] = b"a3s.use.capability-ref.v1\0";
const CAPABILITY_SCHEMA_DIGEST_DOMAIN: &[u8] = b"a3s.use.capability-schema-digest.v1\0";
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
        /// Digest of the exact signed Runtime release descriptor that
        /// supplies the Tool payload.  It is optional for compatibility with
        /// host-only descriptors; the strict Control projector requires it
        /// before exposing a Tool to an agent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_descriptor_digest: Option<String>,
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
    /// Path-free A3S Flow metadata. Visible only to consumers that accepted
    /// the `flow` extension; never compiled into MCP Tool routes.
    Flow {
        engine: String,
        runtime: String,
        export_name: String,
        artifact_digest: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        requires_tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        requires_mcp: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        requires_knowledge: Vec<String>,
    },
    /// Path-free OKF Knowledge metadata. Visible only to consumers that
    /// accepted the `knowledge` extension.
    Knowledge {
        bundle_digest: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        okf_version: Option<String>,
    },
    /// Path-free UI surface metadata. Visible only to consumers that accepted
    /// the `ui` extension. Display title and description live on the outer
    /// descriptor; this variant carries only UI-specific projection fields.
    Ui {
        icon: String,
        order: i32,
        entry_digest: String,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bind_tools: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bind_mcp: Vec<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        bind_flows: Vec<String>,
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
    /// Optional A3S metadata required by a consumer before this descriptor
    /// can be exposed. An empty set means the capability is usable by every
    /// standard MCP consumer.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub required_extensions: Vec<CapabilityConsumerExtension>,
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
    #[serde(default)]
    required_extensions: Vec<CapabilityConsumerExtension>,
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
    runtime_descriptor_digest: Option<String>,
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
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    runtime: Option<String>,
    #[serde(default)]
    export_name: Option<String>,
    #[serde(default)]
    artifact_digest: Option<String>,
    #[serde(default)]
    requires_tools: Option<Vec<String>>,
    #[serde(default)]
    requires_mcp: Option<Vec<String>>,
    #[serde(default)]
    requires_knowledge: Option<Vec<String>>,
    #[serde(default)]
    bundle_digest: Option<String>,
    #[serde(default)]
    okf_version: Option<String>,
    #[serde(default)]
    icon: Option<String>,
    #[serde(default)]
    order: Option<i32>,
    #[serde(default)]
    entry_digest: Option<String>,
    #[serde(default)]
    bind_tools: Option<Vec<String>>,
    #[serde(default)]
    bind_mcp: Option<Vec<String>>,
    #[serde(default)]
    bind_flows: Option<Vec<String>>,
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
        "requiredExtensions",
        "publication",
        "kind",
        "name",
        "inputSchema",
        "outputSchema",
        "annotations",
        "runtimeDescriptorDigest",
        "serverName",
        "transport",
        "protocolVersion",
        "uri",
        "mimeType",
        "size",
        "arguments",
        "engine",
        "runtime",
        "exportName",
        "artifactDigest",
        "requiresTools",
        "requiresMcp",
        "requiresKnowledge",
        "bundleDigest",
        "okfVersion",
        "icon",
        "order",
        "entryDigest",
        "bindTools",
        "bindMcp",
        "bindFlows",
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
                || has_extension_kind_fields(&wire)
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
                runtime_descriptor_digest: wire.runtime_descriptor_digest,
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
                || wire.runtime_descriptor_digest.is_some()
                || has_extension_kind_fields(&wire)
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
                || wire.runtime_descriptor_digest.is_some()
                || wire.server_name.is_some()
                || wire.transport.is_some()
                || wire.protocol_version.is_some()
                || wire.arguments.is_some()
                || has_extension_kind_fields(&wire)
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
                || wire.runtime_descriptor_digest.is_some()
                || wire.server_name.is_some()
                || wire.transport.is_some()
                || wire.protocol_version.is_some()
                || wire.uri.is_some()
                || wire.mime_type.is_some()
                || wire.size.is_some()
                || has_extension_kind_fields(&wire)
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
        "flow" => {
            if has_standard_mcp_kind_fields(&wire)
                || wire.bundle_digest.is_some()
                || wire.okf_version.is_some()
                || wire.icon.is_some()
                || wire.order.is_some()
                || wire.entry_digest.is_some()
                || wire.bind_tools.is_some()
                || wire.bind_mcp.is_some()
                || wire.bind_flows.is_some()
            {
                return Err(
                    "A Flow descriptor contains fields for another capability kind.".to_owned(),
                );
            }
            CapabilityDescriptorKind::Flow {
                engine: wire
                    .engine
                    .ok_or_else(|| "A Flow descriptor requires `engine`.".to_owned())?,
                runtime: wire
                    .runtime
                    .ok_or_else(|| "A Flow descriptor requires `runtime`.".to_owned())?,
                export_name: wire
                    .export_name
                    .ok_or_else(|| "A Flow descriptor requires `exportName`.".to_owned())?,
                artifact_digest: wire
                    .artifact_digest
                    .ok_or_else(|| "A Flow descriptor requires `artifactDigest`.".to_owned())?,
                requires_tools: wire.requires_tools.unwrap_or_default(),
                requires_mcp: wire.requires_mcp.unwrap_or_default(),
                requires_knowledge: wire.requires_knowledge.unwrap_or_default(),
            }
        }
        "knowledge" => {
            if has_standard_mcp_kind_fields(&wire)
                || wire.engine.is_some()
                || wire.runtime.is_some()
                || wire.export_name.is_some()
                || wire.artifact_digest.is_some()
                || wire.requires_tools.is_some()
                || wire.requires_mcp.is_some()
                || wire.requires_knowledge.is_some()
                || wire.icon.is_some()
                || wire.order.is_some()
                || wire.entry_digest.is_some()
                || wire.bind_tools.is_some()
                || wire.bind_mcp.is_some()
                || wire.bind_flows.is_some()
            {
                return Err(
                    "A Knowledge descriptor contains fields for another capability kind."
                        .to_owned(),
                );
            }
            CapabilityDescriptorKind::Knowledge {
                bundle_digest: wire
                    .bundle_digest
                    .ok_or_else(|| "A Knowledge descriptor requires `bundleDigest`.".to_owned())?,
                okf_version: wire.okf_version,
            }
        }
        "ui" => {
            if has_standard_mcp_kind_fields(&wire)
                || wire.engine.is_some()
                || wire.runtime.is_some()
                || wire.export_name.is_some()
                || wire.artifact_digest.is_some()
                || wire.requires_tools.is_some()
                || wire.requires_mcp.is_some()
                || wire.requires_knowledge.is_some()
                || wire.bundle_digest.is_some()
                || wire.okf_version.is_some()
            {
                return Err(
                    "A UI descriptor contains fields for another capability kind.".to_owned(),
                );
            }
            CapabilityDescriptorKind::Ui {
                icon: wire
                    .icon
                    .ok_or_else(|| "A UI descriptor requires `icon`.".to_owned())?,
                order: wire
                    .order
                    .ok_or_else(|| "A UI descriptor requires `order`.".to_owned())?,
                entry_digest: wire
                    .entry_digest
                    .ok_or_else(|| "A UI descriptor requires `entryDigest`.".to_owned())?,
                bind_tools: wire.bind_tools.unwrap_or_default(),
                bind_mcp: wire.bind_mcp.unwrap_or_default(),
                bind_flows: wire.bind_flows.unwrap_or_default(),
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
        required_extensions: wire.required_extensions,
        publication: wire.publication,
        capability,
    })
}

fn has_standard_mcp_kind_fields(wire: &CapabilityDescriptorWire) -> bool {
    wire.name.is_some()
        || wire.input_schema.is_some()
        || wire.output_schema.is_some()
        || wire.annotations.is_some()
        || wire.runtime_descriptor_digest.is_some()
        || wire.server_name.is_some()
        || wire.transport.is_some()
        || wire.protocol_version.is_some()
        || wire.uri.is_some()
        || wire.mime_type.is_some()
        || wire.size.is_some()
        || wire.arguments.is_some()
}

fn has_extension_kind_fields(wire: &CapabilityDescriptorWire) -> bool {
    wire.engine.is_some()
        || wire.runtime.is_some()
        || wire.export_name.is_some()
        || wire.artifact_digest.is_some()
        || wire.requires_tools.is_some()
        || wire.requires_mcp.is_some()
        || wire.requires_knowledge.is_some()
        || wire.bundle_digest.is_some()
        || wire.okf_version.is_some()
        || wire.icon.is_some()
        || wire.order.is_some()
        || wire.entry_digest.is_some()
        || wire.bind_tools.is_some()
        || wire.bind_mcp.is_some()
        || wire.bind_flows.is_some()
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

include!("capability_gateway_descriptor.rs");

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

include!("capability_gateway_catalog.rs");
#[cfg(test)]
#[path = "capability_gateway_tests.rs"]
mod tests;
