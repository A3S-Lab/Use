//! Standard MCP adapter for the path-free Capability Gateway contract.
//!
//! The adapter is intentionally a thin protocol boundary.  It owns an
//! immutable, generation-bound catalog and delegates every invocation to an
//! injected provider.  The provider resolves the opaque invocation reference
//! inside the host authority; no client-supplied path, package root, endpoint,
//! or credential is accepted by this module.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{
    CapabilityConsumerNegotiation, CapabilityConsumerProfile, CapabilityDescriptionProof,
    CapabilityDescriptor, CapabilityDescriptorKind, CapabilityGatewayCatalog,
    CapabilityPromptArgument, CapabilityToolAnnotations, ResourceRef, UseError, UseResult,
};
use async_trait::async_trait;
use base64::Engine as _;
use jsonschema::{Draft, Validator};
use rmcp::handler::server::router::tool::{ToolRoute, ToolRouter};
use rmcp::handler::server::tool::ToolCallContext;
use rmcp::model::AnnotateAble;
use rmcp::model::{
    CallToolRequestParam, CallToolResult, GetPromptRequestParam, GetPromptResult, Implementation,
    JsonObject, ListPromptsResult, ListResourcesResult, ListToolsResult, PaginatedRequestParam,
    Prompt, PromptArgument, PromptMessageContent, RawResource, ReadResourceRequestParam,
    ReadResourceResult, Resource, ResourceContents, ServerCapabilities, ServerInfo, Tool,
    ToolAnnotations,
};
use rmcp::{ServerHandler, ServiceExt};
use serde_json::Value;
use tokio::sync::OwnedSemaphorePermit;

use crate::capability_registry::{CapabilitySnapshotCursor, CapabilitySnapshotLease};

mod admission;
mod http;
mod resolver;

pub use admission::CapabilityGatewayLimits;
use admission::{AdmissionFailure, GatewayAdmission};
pub use http::CapabilityGatewayHttpConfig;
pub use resolver::{
    CapabilityGatewayInvocation, CapabilityGatewayInvocationFactory,
    CapabilityGatewayInvocationLease, CapabilityGatewayInvocationResolver,
    CapabilityGatewayRegistryResolver, CapabilityGatewayResolvedProvider,
};

const MCP_ERROR: &str = "use.plugin.capability_gateway_mcp_invalid";
const MCP_SCHEMA_ERROR: &str = "use.plugin.capability_gateway_schema_violation";
const MCP_AUTHORIZATION_ERROR: &str = "use.plugin.capability_gateway_forbidden";
const MCP_INVOCATION_ERROR: &str = "use.plugin.capability_gateway_invocation_failed";
const MCP_RATE_LIMIT_ERROR: &str = "use.plugin.capability_gateway_rate_limited";
const MCP_RESOURCE_ERROR: &str = "use.plugin.capability_gateway_resource_failed";
const MCP_PROMPT_ERROR: &str = "use.plugin.capability_gateway_prompt_failed";
const MAX_CAPABILITY_VALUE_BYTES: usize = 256 * 1024;
const MAX_CAPABILITY_VALUE_DEPTH: usize = 32;
const MAX_CAPABILITY_VALUE_ELEMENTS: usize = 4_096;
const MAX_CAPABILITY_RESOURCE_SIZE: u32 = 256 * 1024;
const MAX_CAPABILITY_PRINCIPAL_BYTES: usize = 256;
const MAX_DISCOVERY_ITEMS_PER_PAGE: usize = 64;
const MAX_DISCOVERY_CURSOR_BYTES: usize = 16;

/// Host-authenticated identity supplied to a Capability Gateway provider.
///
/// A principal is created from host configuration after the HTTP bearer
/// credential has been verified. It is never decoded from an MCP argument or
/// accepted from an agent-visible descriptor.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CapabilityGatewayPrincipal(String);

impl CapabilityGatewayPrincipal {
    /// Parse a bounded, portable principal identity.
    pub fn parse(value: impl Into<String>) -> UseResult<Self> {
        let value = value.into();
        if value.is_empty()
            || value.len() > MAX_CAPABILITY_PRINCIPAL_BYTES
            || !value.is_ascii()
            || !value
                .as_bytes()
                .first()
                .is_some_and(u8::is_ascii_alphanumeric)
            || !value.bytes().all(|byte| {
                byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/' | b'@')
            })
        {
            return Err(mcp_error(
                "The Capability Gateway principal identity is empty or invalid.",
            ));
        }
        Ok(Self(value))
    }

    /// Return the stable host-configured identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Transport context visible only to the host authorization and invocation
/// provider. The value is never serialized into MCP discovery or results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CapabilityGatewayTransport {
    Stdio,
    StreamableHttp,
}

/// Trusted request context assembled by the Gateway boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewayRequestContext {
    transport: CapabilityGatewayTransport,
    principal: Option<CapabilityGatewayPrincipal>,
}

impl CapabilityGatewayRequestContext {
    pub fn transport(&self) -> CapabilityGatewayTransport {
        self.transport
    }

    /// Return the authenticated principal, when the embedding host configured
    /// one for this endpoint. An absent principal is intentionally distinct
    /// from an anonymous string and should normally be denied by policy.
    pub fn principal(&self) -> Option<&CapabilityGatewayPrincipal> {
        self.principal.as_ref()
    }

    pub(crate) fn stdio() -> Self {
        Self {
            transport: CapabilityGatewayTransport::Stdio,
            principal: None,
        }
    }

    pub(crate) fn streamable_http(principal: Option<CapabilityGatewayPrincipal>) -> Self {
        Self {
            transport: CapabilityGatewayTransport::StreamableHttp,
            principal,
        }
    }
}

#[derive(Clone)]
struct CompiledCapabilitySchema {
    validator: Arc<Validator>,
}

impl std::fmt::Debug for CompiledCapabilitySchema {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CompiledCapabilitySchema")
            .finish_non_exhaustive()
    }
}

impl CompiledCapabilitySchema {
    fn compile(schema: &Value) -> UseResult<Self> {
        let validator = jsonschema::options()
            .with_draft(Draft::Draft202012)
            .with_retriever(NoExternalSchemaRetriever)
            .build(schema)
            .map_err(|_| {
                mcp_error(
                    "The Capability Gateway schema cannot be compiled by the fixed validator.",
                )
            })?;
        Ok(Self {
            validator: Arc::new(validator),
        })
    }

    fn validate(&self, value: &Value) -> UseResult<()> {
        let encoded = serde_json::to_vec(value).map_err(|_| schema_value_error())?;
        if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
            return Err(schema_value_error());
        }
        validate_value_bounds(value, 0)?;
        if self.validator.is_valid(value) {
            Ok(())
        } else {
            Err(schema_value_error())
        }
    }
}

/// Capability schemas are self-contained contract data.  An agent-visible
/// descriptor must never make validator construction read a URL or local
/// file, even if a future schema keyword introduces another reference form.
#[derive(Debug, Clone, Copy)]
struct NoExternalSchemaRetriever;

impl jsonschema::Retrieve for NoExternalSchemaRetriever {
    fn retrieve(
        &self,
        _uri: &jsonschema::Uri<String>,
    ) -> Result<Value, Box<dyn std::error::Error + Send + Sync>> {
        Err("external Capability Gateway schema retrieval is disabled".into())
    }
}

#[derive(Clone)]
struct CapabilityGatewayTool {
    descriptor_index: usize,
    input_schema: CompiledCapabilitySchema,
    output_schema: CompiledCapabilitySchema,
}

#[derive(Clone)]
struct CapabilityGatewayResource {
    descriptor_index: usize,
    resource: Resource,
}

#[derive(Clone)]
struct CapabilityGatewayPrompt {
    descriptor_index: usize,
    prompt: Prompt,
    arguments: Vec<CapabilityPromptArgument>,
}

/// Host-owned invocation boundary for a Capability Gateway Tool.
///
/// Implementations must resolve `descriptor.invocation_ref` against their
/// private, generation-fenced authority.  The `arguments` value contains only
/// the MCP tool arguments; it never contains an invocation or endpoint
/// reference supplied by the client.
#[async_trait]
pub trait CapabilityGatewayInvocationProvider: Send + Sync {
    /// Authorize one already schema-validated call against the host's private
    /// policy and principal context. Implementations must make the policy
    /// explicit, fail closed, and must not return package-controlled
    /// diagnostics to the caller. There is intentionally no default
    /// implementation: a provider cannot accidentally turn an absent policy
    /// into an allow-all Gateway.
    async fn authorize(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: &Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<()>;

    async fn invoke(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value>;

    /// Resolve and materialize one catalog-authorized MCP resource. Providers
    /// that do not support resources remain fail-closed by default.
    async fn read_resource(
        &self,
        _descriptor: &CapabilityDescriptor,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Vec<ResourceContents>> {
        Err(UseError::new(
            MCP_RESOURCE_ERROR,
            "The Capability Gateway resource provider is not configured.",
        ))
    }

    /// Resolve and materialize one catalog-authorized MCP prompt. Providers
    /// that do not support prompts remain fail-closed by default.
    async fn get_prompt(
        &self,
        _descriptor: &CapabilityDescriptor,
        _arguments: Value,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<GetPromptResult> {
        Err(UseError::new(
            MCP_PROMPT_ERROR,
            "The Capability Gateway prompt provider is not configured.",
        ))
    }

    /// Authorize and invoke one call as one provider operation.
    ///
    /// The default preserves the two-hook compatibility contract. Providers
    /// that resolve a generation-fenced invocation should override this method
    /// so the authorization decision and the leased invocation use the same
    /// resolved binding without a second lookup.
    async fn authorize_and_invoke(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<Value, CapabilityGatewayInvocationFailure> {
        self.authorize(descriptor, &arguments, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Authorization)?;
        self.invoke(descriptor, arguments, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Invocation)
    }

    /// Authorize and read a resource as one provider operation.
    async fn authorize_and_read_resource(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<Vec<ResourceContents>, CapabilityGatewayInvocationFailure> {
        self.authorize(descriptor, &Value::Null, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Authorization)?;
        self.read_resource(descriptor, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Invocation)
    }

    /// Authorize and get a prompt as one provider operation.
    async fn authorize_and_get_prompt(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<GetPromptResult, CapabilityGatewayInvocationFailure> {
        self.authorize(descriptor, &arguments, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Authorization)?;
        self.get_prompt(descriptor, arguments, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Invocation)
    }
}

/// Internal classification used to keep authorization failures separate from
/// invocation failures while both remain secret-free at the MCP boundary.
#[derive(Debug)]
pub enum CapabilityGatewayInvocationFailure {
    Authorization(UseError),
    Invocation(UseError),
}

/// Host-owned options for composing a live Capability Gateway.
///
/// The options are deliberately separate from package descriptors.  A
/// consumer negotiation and admission limits are endpoint policy, while the
/// invocation factory remains the host's receipt/Runtime/Grant authority.
/// Keeping all three values in one validated input makes it harder for an
/// embedding host to accidentally construct the catalog with one policy and
/// the live resolver with another.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewayCompositionOptions {
    pub negotiation: CapabilityConsumerNegotiation,
    pub limits: CapabilityGatewayLimits,
}

impl Default for CapabilityGatewayCompositionOptions {
    fn default() -> Self {
        Self {
            negotiation: CapabilityConsumerNegotiation::generic_mcp(),
            limits: CapabilityGatewayLimits::default(),
        }
    }
}

impl CapabilityGatewayCompositionOptions {
    /// Construct explicit endpoint policy for one live Gateway composition.
    pub fn new(
        negotiation: CapabilityConsumerNegotiation,
        limits: CapabilityGatewayLimits,
    ) -> Self {
        Self {
            negotiation,
            limits,
        }
    }
}

/// Standard MCP server backed by an immutable Capability Gateway catalog.
#[derive(Clone)]
pub struct CapabilityGatewayMcpServer {
    catalog: Arc<CapabilityGatewayCatalog>,
    consumer_negotiation: Arc<CapabilityConsumerNegotiation>,
    provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    tools: Arc<BTreeMap<String, CapabilityGatewayTool>>,
    resources: Arc<BTreeMap<String, CapabilityGatewayResource>>,
    prompts: Arc<BTreeMap<String, CapabilityGatewayPrompt>>,
    tool_router: ToolRouter<Self>,
    admission: Arc<GatewayAdmission>,
    transport: CapabilityGatewayTransport,
    /// When present, this RAII lease pins every callable package generation
    /// for the lifetime of the MCP service (including cloned session handles).
    snapshot_lease: Option<Arc<CapabilitySnapshotLease>>,
}

impl std::fmt::Debug for CapabilityGatewayMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayMcpServer")
            .field("catalog", &self.catalog)
            .field("consumer_negotiation", &self.consumer_negotiation)
            .field("has_snapshot_lease", &self.snapshot_lease.is_some())
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

impl CapabilityGatewayMcpServer {
    /// Compose an MCP adapter and freeze the catalog for its lifetime.
    pub fn new(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    ) -> UseResult<Self> {
        Self::build(
            catalog,
            provider,
            CapabilityConsumerNegotiation::generic_mcp(),
            None,
            CapabilityGatewayLimits::default(),
        )
    }

    /// Compose an MCP adapter with explicit bounded invocation admission.
    pub fn with_limits(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        limits: CapabilityGatewayLimits,
    ) -> UseResult<Self> {
        Self::build(
            catalog,
            provider,
            CapabilityConsumerNegotiation::generic_mcp(),
            None,
            limits,
        )
    }

    /// Compose a Gateway for an explicit, already completed consumer
    /// negotiation. The negotiation is retained with the immutable server so
    /// a caller cannot accidentally reuse an A3S extension decision with a
    /// different consumer or silently downgrade a requested extension.
    pub fn with_consumer_negotiation(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        negotiation: CapabilityConsumerNegotiation,
    ) -> UseResult<Self> {
        Self::build(
            catalog,
            provider,
            negotiation,
            None,
            CapabilityGatewayLimits::default(),
        )
    }

    /// Compose a Gateway for an explicit consumer negotiation and bounded
    /// invocation admission.
    pub fn with_consumer_negotiation_and_limits(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        negotiation: CapabilityConsumerNegotiation,
        limits: CapabilityGatewayLimits,
    ) -> UseResult<Self> {
        Self::build(catalog, provider, negotiation, None, limits)
    }

    /// Compose a Gateway over an exact Use capability snapshot lease.
    ///
    /// The lease is retained by every clone of the server and is released only
    /// after the MCP service (and all of its sessions) stop. Construction fails
    /// closed when the immutable catalog is not bound to the same installation
    /// and package-generation identities as the lease. Reference resolution
    /// remains host-owned by the injected provider.
    pub fn with_snapshot_lease(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        lease: CapabilitySnapshotLease,
    ) -> UseResult<Self> {
        validate_snapshot_binding(&catalog, &lease)?;
        Self::build(
            catalog,
            provider,
            CapabilityConsumerNegotiation::generic_mcp(),
            Some(Arc::new(lease)),
            CapabilityGatewayLimits::default(),
        )
    }

    /// Compose a leased Gateway with explicit bounded invocation admission.
    pub fn with_snapshot_lease_and_limits(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        lease: CapabilitySnapshotLease,
        limits: CapabilityGatewayLimits,
    ) -> UseResult<Self> {
        validate_snapshot_binding(&catalog, &lease)?;
        Self::build(
            catalog,
            provider,
            CapabilityConsumerNegotiation::generic_mcp(),
            Some(Arc::new(lease)),
            limits,
        )
    }

    /// Compose a leased Gateway for an explicit consumer negotiation.
    pub fn with_snapshot_lease_and_consumer_negotiation(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        lease: CapabilitySnapshotLease,
        negotiation: CapabilityConsumerNegotiation,
    ) -> UseResult<Self> {
        validate_snapshot_binding(&catalog, &lease)?;
        Self::build(
            catalog,
            provider,
            negotiation,
            Some(Arc::new(lease)),
            CapabilityGatewayLimits::default(),
        )
    }

    /// Compose a leased Gateway for an explicit consumer negotiation and
    /// bounded invocation admission.
    pub fn with_snapshot_lease_and_consumer_negotiation_and_limits(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        lease: CapabilitySnapshotLease,
        negotiation: CapabilityConsumerNegotiation,
        limits: CapabilityGatewayLimits,
    ) -> UseResult<Self> {
        validate_snapshot_binding(&catalog, &lease)?;
        Self::build(
            catalog,
            provider,
            negotiation,
            Some(Arc::new(lease)),
            limits,
        )
    }

    /// Bind a catalog to the current Use publication and acquire its exact
    /// generation lease. `None` means the publication changed or a required
    /// generation is already draining; callers should refresh the catalog and
    /// retry instead of serving a mixed snapshot.
    pub async fn from_registry(
        registry: &crate::capability_registry::CapabilityRegistry,
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    ) -> UseResult<Option<Self>> {
        let snapshot = registry.snapshot().await?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease(catalog, provider, lease).map(Some)
    }

    /// Bind a catalog to the current Use publication and an explicit consumer
    /// negotiation. `None` means the publication changed or a required
    /// generation is already draining.
    pub async fn from_registry_with_consumer_negotiation(
        registry: &crate::capability_registry::CapabilityRegistry,
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        negotiation: CapabilityConsumerNegotiation,
    ) -> UseResult<Option<Self>> {
        negotiation.validate()?;
        let snapshot = registry.snapshot().await?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease_and_consumer_negotiation(catalog, provider, lease, negotiation)
            .map(Some)
    }

    /// Build and bind a Gateway catalog from one stable Use snapshot.
    ///
    /// The descriptor source remains host-owned: callers must obtain signed,
    /// schema-checked descriptions from their package authority and pass only
    /// the descriptors intended for this consumer. The snapshot helper checks
    /// their package and publication evidence before this method acquires the
    /// exact RAII lease. `None` means the publication changed or became
    /// unleaseable while the binding was being established.
    pub async fn from_registry_snapshot(
        registry: &crate::capability_registry::CapabilityRegistry,
        descriptors: Vec<CapabilityDescriptor>,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    ) -> UseResult<Option<Self>> {
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog(descriptors)?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease(catalog, provider, lease).map(Some)
    }

    /// Build and bind a Gateway catalog from one stable Use snapshot for an
    /// explicit consumer negotiation.
    pub async fn from_registry_snapshot_with_consumer_negotiation(
        registry: &crate::capability_registry::CapabilityRegistry,
        descriptors: Vec<CapabilityDescriptor>,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        negotiation: CapabilityConsumerNegotiation,
    ) -> UseResult<Option<Self>> {
        negotiation.validate()?;
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog(descriptors)?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease_and_consumer_negotiation(catalog, provider, lease, negotiation)
            .map(Some)
    }

    /// Build and bind a Gateway from descriptions that a host has verified
    /// against its signed Registry publication.  This is the preferred
    /// production constructor; the descriptor-only variant remains useful
    /// for embedding hosts that perform an equivalent verification in a
    /// private type boundary.
    pub async fn from_verified_registry_snapshot(
        registry: &crate::capability_registry::CapabilityRegistry,
        proofs: Vec<CapabilityDescriptionProof>,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    ) -> UseResult<Option<Self>> {
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog_from_verified_descriptions(proofs)?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease(catalog, provider, lease).map(Some)
    }

    /// Build and bind a Gateway from verified descriptions for an explicit
    /// consumer negotiation.
    pub async fn from_verified_registry_snapshot_with_consumer_negotiation(
        registry: &crate::capability_registry::CapabilityRegistry,
        proofs: Vec<CapabilityDescriptionProof>,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        negotiation: CapabilityConsumerNegotiation,
    ) -> UseResult<Option<Self>> {
        negotiation.validate()?;
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog_from_verified_descriptions(proofs)?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease_and_consumer_negotiation(catalog, provider, lease, negotiation)
            .map(Some)
    }

    /// Build a production Gateway from host-verified descriptions and a live
    /// invocation factory.
    ///
    /// This is the preferred composition entry point for embedding hosts. It
    /// observes one immutable Registry snapshot, projects only the supplied
    /// verified descriptions, captures the same snapshot cursor for the live
    /// resolver, and acquires the exact RAII lease before returning a server.
    /// A publication race or an already-draining package returns `None`; the
    /// host should refresh its proofs and retry. The factory is still the
    /// host-owned receipt/Runtime/Grant boundary and receives a per-call lease
    /// for every opaque invocation reference.
    pub async fn from_verified_registry_snapshot_with_factory(
        registry: &crate::capability_registry::CapabilityRegistry,
        proofs: Vec<CapabilityDescriptionProof>,
        factory: Arc<dyn CapabilityGatewayInvocationFactory>,
    ) -> UseResult<Option<Self>> {
        Self::from_verified_registry_snapshot_with_factory_and_options(
            registry,
            proofs,
            factory,
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
    }

    /// Build a production Gateway with explicit negotiation and admission
    /// policy. All policy is validated and retained by the returned server.
    pub async fn from_verified_registry_snapshot_with_factory_and_options(
        registry: &crate::capability_registry::CapabilityRegistry,
        proofs: Vec<CapabilityDescriptionProof>,
        factory: Arc<dyn CapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<Self>> {
        options.negotiation.validate()?;
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog_from_verified_descriptions(proofs)?;

        // The resolver and the server lease are both bound to this exact
        // cursor. The resolver re-acquires a short per-call lease, while the
        // server lease keeps the published package generations callable for
        // the lifetime of the MCP service and its clones.
        let resolver = CapabilityGatewayRegistryResolver::new(
            registry.clone(),
            snapshot.cursor().clone(),
            factory,
        )?;
        let provider = Arc::new(CapabilityGatewayResolvedProvider::new(Arc::new(resolver)));
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        let CapabilityGatewayCompositionOptions {
            negotiation,
            limits,
        } = options;
        Self::with_snapshot_lease_and_consumer_negotiation_and_limits(
            catalog,
            provider,
            lease,
            negotiation,
            limits,
        )
        .map(Some)
    }

    fn build(
        catalog: CapabilityGatewayCatalog,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        consumer_negotiation: CapabilityConsumerNegotiation,
        snapshot_lease: Option<Arc<CapabilitySnapshotLease>>,
        limits: CapabilityGatewayLimits,
    ) -> UseResult<Self> {
        consumer_negotiation.validate()?;
        // Keep the negotiated view as part of the immutable server state. A
        // descriptor requiring an extension that this consumer did not
        // explicitly accept must disappear from both discovery and direct
        // invocation routing.
        let catalog = catalog.for_consumer(&consumer_negotiation)?;
        let catalog = Arc::new(catalog);
        let tools = Arc::new(compile_tools(&catalog)?);
        let resources = Arc::new(compile_resources(&catalog)?);
        let prompts = Arc::new(compile_prompts(&catalog)?);
        let tool_router = frozen_tool_router(&catalog)?;
        let admission = Arc::new(GatewayAdmission::new(limits)?);
        Ok(Self {
            catalog,
            consumer_negotiation: Arc::new(consumer_negotiation),
            provider,
            tools,
            resources,
            prompts,
            tool_router,
            admission,
            transport: CapabilityGatewayTransport::Stdio,
            snapshot_lease,
        })
    }

    /// Return the exact immutable catalog used by this server.
    pub fn catalog(&self) -> &CapabilityGatewayCatalog {
        &self.catalog
    }

    /// Return the immutable consumer negotiation bound to this Gateway.
    pub fn consumer_negotiation(&self) -> &CapabilityConsumerNegotiation {
        &self.consumer_negotiation
    }

    /// Return the negotiated consumer profile bound to this Gateway.
    pub fn consumer_profile(&self) -> &CapabilityConsumerProfile {
        self.consumer_negotiation.profile()
    }

    /// Return the exact lease cursor when this server is bound to a live Use
    /// snapshot. A contract-only server returns `None`.
    pub fn snapshot_cursor(&self) -> Option<&CapabilitySnapshotCursor> {
        self.snapshot_lease
            .as_deref()
            .map(CapabilitySnapshotLease::cursor)
    }

    /// Serve standard MCP framing over stdin/stdout until the peer
    /// disconnects.  No A3S-specific JSON-RPC dialect is introduced.
    pub async fn serve_stdio(self) -> UseResult<()> {
        let service = self
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|_| mcp_error("Failed to start Capability Gateway MCP."))?;
        service
            .waiting()
            .await
            .map_err(|_| mcp_error("Capability Gateway MCP stopped with an error."))?;
        Ok(())
    }

    fn request_context(
        &self,
        request_context: &rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CapabilityGatewayRequestContext, rmcp::ErrorData> {
        match self.transport {
            CapabilityGatewayTransport::Stdio => Ok(CapabilityGatewayRequestContext::stdio()),
            CapabilityGatewayTransport::StreamableHttp => request_context
                .extensions
                .get::<axum::http::request::Parts>()
                .and_then(|parts| parts.extensions.get::<CapabilityGatewayRequestContext>())
                .cloned()
                .ok_or_else(|| {
                    rmcp::ErrorData::internal_error(
                        "Capability Gateway HTTP request context is missing.",
                        None,
                    )
                }),
        }
    }

    pub(crate) fn with_transport(mut self, transport: CapabilityGatewayTransport) -> Self {
        self.transport = transport;
        self
    }

    async fn dispatch(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tool = self.tools.get(name).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "Capability Gateway Tool is not part of the immutable catalog.",
                None,
            )
        })?;
        let _permit = match self.admission.try_acquire() {
            Ok(permit) => permit,
            Err(AdmissionFailure::InFlight | AdmissionFailure::RateLimited) => {
                return Ok(structured_error(
                    MCP_RATE_LIMIT_ERROR,
                    "The Capability Gateway is temporarily rate limited.",
                ));
            }
            Err(AdmissionFailure::StatePoisoned) => {
                return Ok(structured_error(
                    MCP_RATE_LIMIT_ERROR,
                    "The Capability Gateway admission state is unavailable.",
                ));
            }
        };
        let descriptor = self
            .catalog
            .descriptors()
            .get(tool.descriptor_index)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "Capability Gateway route index is inconsistent with its catalog.",
                    None,
                )
            })?;
        let arguments = Value::Object(arguments.unwrap_or_default());
        tool.input_schema.validate(&arguments).map_err(|_| {
            rmcp::ErrorData::invalid_params(
                "Capability Gateway Tool arguments do not satisfy the published schema.",
                None,
            )
        })?;
        let result = match self
            .provider
            .authorize_and_invoke(descriptor, arguments, context)
            .await
        {
            Ok(value) => Ok(value),
            Err(CapabilityGatewayInvocationFailure::Authorization(_)) => {
                return Ok(structured_error(
                    MCP_AUTHORIZATION_ERROR,
                    "The Capability Gateway denied this invocation.",
                ));
            }
            Err(CapabilityGatewayInvocationFailure::Invocation(error)) => Err(error),
        };
        Ok(tool_result(result, &tool.output_schema))
    }
}

impl ServerHandler for CapabilityGatewayMcpServer {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities {
            prompts: (!self.prompts.is_empty()).then(Default::default),
            resources: (!self.resources.is_empty()).then(Default::default),
            tools: Some(Default::default()),
            ..Default::default()
        };
        ServerInfo {
            capabilities,
            server_info: Implementation {
                name: "a3s-use-capability-gateway".to_owned(),
                title: Some("A3S Use Capability Gateway".to_owned()),
                version: env!("CARGO_PKG_VERSION").to_owned(),
                icons: None,
                website_url: Some("https://github.com/A3S-Lab/Use".to_owned()),
            },
            instructions: Some(
                "The catalog is immutable and generation-bound. Tool calls are resolved by the host from opaque references; client-supplied paths, package roots, endpoints, and credentials are not accepted."
                    .to_owned(),
            ),
            ..Default::default()
        }
    }

    /// Dispatch Tool calls through the frozen router. This is the small
    /// portion normally generated by rmcp's `tool_handler` macro; it remains
    /// explicit here because the generated `list_tools` implementation is
    /// unbounded and iterates a hash map in an unstable order.
    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        let tool_context = ToolCallContext::new(self, request, request_context);
        self.tool_router.call(tool_context).await
    }

    /// List Tools in stable name order with the same bounded cursor contract
    /// used by Resources and Prompts. `ToolRouter` stores routes in a HashMap,
    /// so sorting is required for a cursor to remain meaningful across calls.
    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        self.request_context(&request_context)?;
        let mut tools = self.tool_router.list_all();
        tools.sort_by(|left, right| left.name.cmp(&right.name));
        let (start, end, next_cursor) =
            discovery_page(request.and_then(|request| request.cursor), tools.len())?;
        let mut result = ListToolsResult::with_all_items(
            tools.into_iter().skip(start).take(end - start).collect(),
        );
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        self.request_context(&request_context)?;
        let (start, end, next_cursor) = discovery_page(
            request.and_then(|request| request.cursor),
            self.resources.len(),
        )?;
        let mut result = ListResourcesResult::with_all_items(
            self.resources
                .values()
                .skip(start)
                .take(end - start)
                .map(|route| route.resource.clone())
                .collect(),
        );
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        self.request_context(&request_context)?;
        let (start, end, next_cursor) = discovery_page(
            request.and_then(|request| request.cursor),
            self.prompts.len(),
        )?;
        let mut result = ListPromptsResult::with_all_items(
            self.prompts
                .values()
                .skip(start)
                .take(end - start)
                .map(|route| route.prompt.clone())
                .collect(),
        );
        result.next_cursor = next_cursor;
        Ok(result)
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let context = self.request_context(&request_context)?;
        let resource_ref = ResourceRef::parse(request.uri.clone()).map_err(|_| {
            rmcp::ErrorData::invalid_params(
                "The requested Capability Gateway resource URI is invalid.",
                None,
            )
        })?;
        let route = self.resources.get(resource_ref.as_str()).ok_or_else(|| {
            rmcp::ErrorData::resource_not_found(
                "The requested Capability Gateway resource is not published.",
                None,
            )
        })?;
        let _permit = content_admission(&self.admission)?;
        let descriptor = self
            .catalog
            .descriptors()
            .get(route.descriptor_index)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "Capability Gateway route index is inconsistent with its catalog.",
                    None,
                )
            })?;
        let contents = match self
            .provider
            .authorize_and_read_resource(descriptor, &context)
            .await
        {
            Ok(contents) => contents,
            Err(CapabilityGatewayInvocationFailure::Authorization(_)) => {
                return Err(rmcp::ErrorData::invalid_request(
                    "The Capability Gateway denied this resource request.",
                    None,
                ));
            }
            Err(CapabilityGatewayInvocationFailure::Invocation(_)) => {
                return Err(rmcp::ErrorData::internal_error(
                    "The Capability Gateway could not read this resource.",
                    None,
                ));
            }
        };
        validate_resource_contents(&request.uri, &contents).map_err(|_| {
            rmcp::ErrorData::internal_error(
                "The Capability Gateway resource provider returned an invalid result.",
                None,
            )
        })?;
        Ok(ReadResourceResult { contents })
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let context = self.request_context(&request_context)?;
        let route = self.prompts.get(&request.name).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "The requested Capability Gateway prompt is not published.",
                None,
            )
        })?;
        let arguments = Value::Object(request.arguments.unwrap_or_default());
        validate_prompt_arguments(&arguments, &route.arguments).map_err(|_| {
            rmcp::ErrorData::invalid_params(
                "The Capability Gateway prompt arguments are invalid.",
                None,
            )
        })?;
        let _permit = content_admission(&self.admission)?;
        let descriptor = self
            .catalog
            .descriptors()
            .get(route.descriptor_index)
            .ok_or_else(|| {
                rmcp::ErrorData::internal_error(
                    "Capability Gateway route index is inconsistent with its catalog.",
                    None,
                )
            })?;
        let result = match self
            .provider
            .authorize_and_get_prompt(descriptor, arguments, &context)
            .await
        {
            Ok(result) => result,
            Err(CapabilityGatewayInvocationFailure::Authorization(_)) => {
                return Err(rmcp::ErrorData::invalid_request(
                    "The Capability Gateway denied this prompt request.",
                    None,
                ));
            }
            Err(CapabilityGatewayInvocationFailure::Invocation(_)) => {
                return Err(rmcp::ErrorData::internal_error(
                    "The Capability Gateway could not generate this prompt.",
                    None,
                ));
            }
        };
        validate_prompt_result(&result, &self.resources).map_err(|_| {
            rmcp::ErrorData::internal_error(
                "The Capability Gateway prompt provider returned an invalid result.",
                None,
            )
        })?;
        Ok(result)
    }
}

fn validate_snapshot_binding(
    catalog: &CapabilityGatewayCatalog,
    lease: &CapabilitySnapshotLease,
) -> UseResult<()> {
    let snapshot = lease.snapshot();
    if snapshot.cursor() != lease.cursor() {
        return Err(mcp_error(
            "The Capability Gateway snapshot lease contains inconsistent cursor evidence.",
        ));
    }
    validate_snapshot_binding_identity(
        catalog,
        snapshot.installation.clone(),
        snapshot.generation,
        lease.cursor(),
    )
}

fn validate_snapshot_binding_identity(
    catalog: &CapabilityGatewayCatalog,
    installation: a3s_use_core::InstallationId,
    generation: u64,
    cursor: &CapabilitySnapshotCursor,
) -> UseResult<()> {
    cursor
        .validate()
        .map_err(|_| mcp_error("The Capability Gateway snapshot lease cursor is invalid."))?;
    if catalog.installation() != &installation
        || cursor.installation != installation
        || cursor.generation != generation
        || catalog.generation() != generation
        || !cursor.is_fully_leasable()
    {
        return Err(mcp_error(
            "The Capability Gateway catalog is not bound to the exact Use snapshot lease.",
        ));
    }

    for descriptor in catalog.descriptors() {
        let package_id = descriptor.package_id.to_string();
        let Some(package) = cursor
            .packages
            .iter()
            .find(|package| package.package_id == package_id)
        else {
            return Err(mcp_error(
                "The Capability Gateway catalog contains a package outside the Use snapshot lease.",
            ));
        };
        if package.lifecycle_generation != descriptor.generation
            || package.package_digest != descriptor.package_digest
            || package.manifest_digest != descriptor.manifest_digest
        {
            return Err(mcp_error(
                "The Capability Gateway descriptor does not match the Use snapshot lease.",
            ));
        }
    }
    Ok(())
}

fn frozen_tool_router(
    catalog: &Arc<CapabilityGatewayCatalog>,
) -> UseResult<ToolRouter<CapabilityGatewayMcpServer>> {
    let mut router = ToolRouter::<CapabilityGatewayMcpServer>::new();
    for descriptor in catalog
        .descriptors()
        .iter()
        .filter(|descriptor| descriptor.is_agent_tool())
    {
        let name = descriptor.tool_name().ok_or_else(|| {
            mcp_error("The Capability Gateway catalog contains a non-Tool route.")
        })?;
        if router.has_route(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate Tool name `{name}`."
            )));
        }
        let tool = mcp_tool(descriptor)?;
        let route_name = name.to_owned();
        router.add_route(ToolRoute::new_dyn(
            tool,
            move |context: rmcp::handler::server::tool::ToolCallContext<
                '_,
                CapabilityGatewayMcpServer,
            >| {
                let route_name = route_name.clone();
                Box::pin(async move {
                    let gateway_context =
                        context.service.request_context(&context.request_context)?;
                    context
                        .service
                        .dispatch(&route_name, context.arguments, &gateway_context)
                        .await
                })
            },
        ));
    }
    Ok(router)
}

fn compile_tools(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayTool>> {
    let mut tools = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        if !descriptor.is_agent_tool() {
            continue;
        }
        let name = descriptor.tool_name().ok_or_else(|| {
            mcp_error("The Capability Gateway catalog contains a non-Tool route.")
        })?;
        if tools.contains_key(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate Tool name `{name}`."
            )));
        }
        let (input_schema, output_schema) = match &descriptor.capability {
            CapabilityDescriptorKind::Tool {
                input_schema,
                output_schema,
                ..
            } => (input_schema, output_schema),
            CapabilityDescriptorKind::McpServer { .. }
            | CapabilityDescriptorKind::Resource { .. }
            | CapabilityDescriptorKind::Prompt { .. } => {
                return Err(mcp_error(
                    "Only Tool descriptors can be compiled for the MCP Gateway.",
                ));
            }
        };
        tools.insert(
            name.to_owned(),
            CapabilityGatewayTool {
                descriptor_index,
                input_schema: CompiledCapabilitySchema::compile(input_schema)?,
                output_schema: CompiledCapabilitySchema::compile(output_schema)?,
            },
        );
    }
    Ok(tools)
}

fn compile_resources(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayResource>> {
    let mut resources = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        let CapabilityDescriptorKind::Resource {
            name,
            uri,
            mime_type,
            size,
        } = &descriptor.capability
        else {
            continue;
        };
        if size.is_some_and(|value| value > MAX_CAPABILITY_RESOURCE_SIZE) {
            return Err(mcp_error(
                "The Capability Gateway resource metadata exceeds its size bound.",
            ));
        }
        if resources.contains_key(uri.as_str()) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate resource URI `{}`.",
                uri.as_str()
            )));
        }
        let mut raw = RawResource::new(uri.as_str(), name.clone());
        raw.title = Some(descriptor.title.clone());
        raw.description = Some(descriptor.description.clone());
        raw.mime_type = mime_type.clone();
        raw.size = *size;
        resources.insert(
            uri.as_str().to_owned(),
            CapabilityGatewayResource {
                descriptor_index,
                resource: raw.no_annotation(),
            },
        );
    }
    Ok(resources)
}

fn compile_prompts(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<BTreeMap<String, CapabilityGatewayPrompt>> {
    let mut prompts = BTreeMap::new();
    for (descriptor_index, descriptor) in catalog.descriptors().iter().enumerate() {
        let CapabilityDescriptorKind::Prompt { name, arguments } = &descriptor.capability else {
            continue;
        };
        if prompts.contains_key(name) {
            return Err(mcp_error(format!(
                "The Capability Gateway catalog contains duplicate prompt name `{name}`."
            )));
        }
        let prompt_arguments = arguments
            .iter()
            .map(|argument| PromptArgument {
                name: argument.name.clone(),
                title: argument.title.clone(),
                description: argument.description.clone(),
                required: Some(argument.required),
            })
            .collect::<Vec<_>>();
        let prompt = Prompt {
            name: name.clone(),
            title: Some(descriptor.title.clone()),
            description: Some(descriptor.description.clone()),
            arguments: (!prompt_arguments.is_empty()).then_some(prompt_arguments),
            icons: None,
        };
        prompts.insert(
            name.clone(),
            CapabilityGatewayPrompt {
                descriptor_index,
                prompt,
                arguments: arguments.clone(),
            },
        );
    }
    Ok(prompts)
}

fn mcp_tool(descriptor: &CapabilityDescriptor) -> UseResult<Tool> {
    let CapabilityDescriptorKind::Tool {
        name,
        input_schema,
        output_schema,
        annotations,
    } = &descriptor.capability
    else {
        return Err(mcp_error("Only Tool descriptors can be exposed over MCP."));
    };
    let input_schema = input_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A Capability Gateway input schema is not an object."))?;
    let output_schema = output_schema
        .as_object()
        .cloned()
        .ok_or_else(|| mcp_error("A Capability Gateway output schema is not an object."))?;
    Ok(Tool {
        name: Cow::Owned(name.clone()),
        title: Some(descriptor.title.clone()),
        description: Some(Cow::Owned(descriptor.description.clone())),
        input_schema: Arc::new(input_schema),
        output_schema: Some(Arc::new(output_schema)),
        annotations: Some(mcp_annotations(*annotations)),
        icons: None,
    })
}

fn mcp_annotations(annotations: CapabilityToolAnnotations) -> ToolAnnotations {
    ToolAnnotations {
        title: None,
        read_only_hint: Some(annotations.read_only_hint),
        destructive_hint: Some(annotations.destructive_hint),
        idempotent_hint: Some(annotations.idempotent_hint),
        open_world_hint: Some(annotations.open_world_hint),
    }
}

fn content_admission(
    admission: &GatewayAdmission,
) -> Result<OwnedSemaphorePermit, rmcp::ErrorData> {
    match admission.try_acquire() {
        Ok(permit) => Ok(permit),
        Err(AdmissionFailure::InFlight | AdmissionFailure::RateLimited) => {
            Err(rmcp::ErrorData::internal_error(
                "The Capability Gateway is temporarily rate limited.",
                None,
            ))
        }
        Err(AdmissionFailure::StatePoisoned) => Err(rmcp::ErrorData::internal_error(
            "The Capability Gateway admission state is unavailable.",
            None,
        )),
    }
}

fn discovery_page(
    cursor: Option<String>,
    item_count: usize,
) -> Result<(usize, usize, Option<String>), rmcp::ErrorData> {
    let start = match cursor {
        None => 0,
        Some(cursor)
            if cursor.len() <= MAX_DISCOVERY_CURSOR_BYTES
                && !cursor.is_empty()
                && cursor.bytes().all(|byte| byte.is_ascii_digit()) =>
        {
            cursor.parse::<usize>().map_err(|_| {
                rmcp::ErrorData::invalid_params(
                    "The Capability Gateway discovery cursor is invalid.",
                    None,
                )
            })?
        }
        Some(_) => {
            return Err(rmcp::ErrorData::invalid_params(
                "The Capability Gateway discovery cursor is invalid.",
                None,
            ));
        }
    };
    if start > item_count {
        return Err(rmcp::ErrorData::invalid_params(
            "The Capability Gateway discovery cursor is outside the catalog.",
            None,
        ));
    }
    let end = start
        .saturating_add(MAX_DISCOVERY_ITEMS_PER_PAGE)
        .min(item_count);
    let next_cursor = (end < item_count).then(|| end.to_string());
    Ok((start, end, next_cursor))
}

fn validate_resource_contents(uri: &str, contents: &[ResourceContents]) -> UseResult<()> {
    if contents.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
        return Err(schema_value_error());
    }
    for content in contents {
        let content_uri = match content {
            ResourceContents::TextResourceContents {
                uri,
                mime_type,
                text,
                ..
            } => {
                validate_content_text(text)?;
                validate_content_mime(mime_type.as_deref())?;
                uri
            }
            ResourceContents::BlobResourceContents {
                uri,
                mime_type,
                blob,
                ..
            } => {
                validate_content_blob(blob)?;
                validate_content_mime(mime_type.as_deref())?;
                uri
            }
        };
        if content_uri != uri || ResourceRef::parse(content_uri.clone()).is_err() {
            return Err(schema_value_error());
        }
    }
    let encoded = serde_json::to_vec(contents).map_err(|_| schema_value_error())?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
        return Err(schema_value_error());
    }
    Ok(())
}

fn validate_prompt_arguments(
    arguments: &Value,
    declarations: &[CapabilityPromptArgument],
) -> UseResult<()> {
    let Value::Object(arguments) = arguments else {
        return Err(schema_value_error());
    };
    let encoded = serde_json::to_vec(arguments).map_err(|_| schema_value_error())?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES {
        return Err(schema_value_error());
    }
    validate_value_bounds(&Value::Object(arguments.clone()), 0)?;
    for key in arguments.keys() {
        if !declarations
            .iter()
            .any(|declaration| declaration.name == *key)
        {
            return Err(schema_value_error());
        }
    }
    if arguments
        .values()
        .any(|value| !value.as_str().is_some_and(valid_content_text))
    {
        return Err(schema_value_error());
    }
    for declaration in declarations {
        if declaration.required && !arguments.contains_key(&declaration.name) {
            return Err(schema_value_error());
        }
    }
    Ok(())
}

fn validate_prompt_result(
    result: &GetPromptResult,
    resources: &BTreeMap<String, CapabilityGatewayResource>,
) -> UseResult<()> {
    let encoded = serde_json::to_vec(result).map_err(|_| schema_value_error())?;
    if encoded.len() > MAX_CAPABILITY_VALUE_BYTES
        || result.messages.len() > MAX_CAPABILITY_VALUE_ELEMENTS
    {
        return Err(schema_value_error());
    }
    if result
        .description
        .as_deref()
        .is_some_and(|description| !valid_content_text(description))
    {
        return Err(schema_value_error());
    }
    for message in &result.messages {
        match &message.content {
            PromptMessageContent::Text { text } => validate_content_text(text)?,
            PromptMessageContent::Image { image } => {
                validate_content_blob(&image.data)?;
                validate_content_text(&image.mime_type)?;
            }
            PromptMessageContent::Resource { resource } => {
                validate_prompt_resource_contents(&resource.resource, resources)?;
            }
            PromptMessageContent::ResourceLink { link } => {
                if ResourceRef::parse(link.uri.clone()).is_err()
                    || !resources.contains_key(link.uri.as_str())
                    || !valid_content_text(&link.name)
                    || link
                        .description
                        .as_deref()
                        .is_some_and(|description| !valid_content_text(description))
                {
                    return Err(schema_value_error());
                }
            }
        }
    }
    Ok(())
}

fn validate_prompt_resource_contents(
    content: &ResourceContents,
    resources: &BTreeMap<String, CapabilityGatewayResource>,
) -> UseResult<()> {
    let uri = match content {
        ResourceContents::TextResourceContents {
            uri,
            mime_type,
            text,
            ..
        } => {
            validate_content_text(text)?;
            validate_content_mime(mime_type.as_deref())?;
            uri
        }
        ResourceContents::BlobResourceContents {
            uri,
            mime_type,
            blob,
            ..
        } => {
            validate_content_blob(blob)?;
            validate_content_mime(mime_type.as_deref())?;
            uri
        }
    };
    if ResourceRef::parse(uri.clone()).is_err() || !resources.contains_key(uri.as_str()) {
        return Err(schema_value_error());
    }
    Ok(())
}

fn validate_content_mime(mime_type: Option<&str>) -> UseResult<()> {
    if mime_type.is_some_and(|value| !valid_content_text(value)) {
        return Err(schema_value_error());
    }
    Ok(())
}

fn validate_content_text(value: &str) -> UseResult<()> {
    if valid_content_text(value) {
        Ok(())
    } else {
        Err(schema_value_error())
    }
}

fn validate_content_blob(value: &str) -> UseResult<()> {
    if !valid_content_text(value) {
        return Err(schema_value_error());
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| schema_value_error())?;
    if decoded.len() > MAX_CAPABILITY_VALUE_BYTES
        || base64::engine::general_purpose::STANDARD.encode(decoded) != value
    {
        return Err(schema_value_error());
    }
    Ok(())
}

fn valid_content_text(value: &str) -> bool {
    value.len() <= MAX_CAPABILITY_VALUE_BYTES && !value.chars().any(char::is_control)
}

fn tool_result(
    result: UseResult<Value>,
    output_schema: &CompiledCapabilitySchema,
) -> CallToolResult {
    match result {
        Ok(value) => match output_schema.validate(&value) {
            Ok(()) => CallToolResult::structured(value),
            Err(_) => structured_error(
                MCP_SCHEMA_ERROR,
                "The Capability Gateway provider returned a value outside the published schema.",
            ),
        },
        Err(_) => structured_error(
            MCP_INVOCATION_ERROR,
            "The Capability Gateway provider could not complete the invocation.",
        ),
    }
}

fn structured_error(code: &str, message: &str) -> CallToolResult {
    CallToolResult::structured_error(serde_json::json!({
        "code": code,
        "message": message,
    }))
}

fn schema_value_error() -> UseError {
    UseError::new(
        MCP_SCHEMA_ERROR,
        "The capability value does not satisfy its published schema.",
    )
}

fn validate_value_bounds(value: &Value, depth: usize) -> UseResult<()> {
    if depth > MAX_CAPABILITY_VALUE_DEPTH {
        return Err(schema_value_error());
    }
    match value {
        Value::Array(values) => {
            if values.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
                return Err(schema_value_error());
            }
            for value in values {
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Object(object) => {
            if object.len() > MAX_CAPABILITY_VALUE_ELEMENTS {
                return Err(schema_value_error());
            }
            for (key, value) in object {
                if key.len() > 4 * 1024 || key.chars().any(char::is_control) {
                    return Err(schema_value_error());
                }
                validate_value_bounds(value, depth + 1)?;
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
    Ok(())
}

fn mcp_error(message: impl Into<String>) -> UseError {
    UseError::new(MCP_ERROR, message)
}

#[cfg(test)]
mod tests;
