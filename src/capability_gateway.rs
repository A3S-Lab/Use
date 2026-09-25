//! Standard MCP adapter for the path-free Capability Gateway contract.
//!
//! The adapter is intentionally a thin protocol boundary.  It owns an
//! immutable, generation-bound catalog and delegates every invocation to an
//! injected provider.  The provider resolves the opaque invocation reference
//! inside the host authority; no client-supplied path, package root, endpoint,
//! or credential is accepted by this module.

use a3s_use_core::{CapabilityConsumerNegotiation, CapabilityDescriptor, UseError, UseResult};
use async_trait::async_trait;
use rmcp::model::{GetPromptResult, ResourceContents};
use serde_json::Value;

mod admission;
mod discovery;
mod http;
mod notifications;
mod protocol;
mod resolver;
mod server;
mod session_factory;
pub use crate::capability_catalog_store::{
    CapabilityGatewayCatalogPublication, CapabilityGatewayCatalogRetentionEntry,
    CapabilityGatewayCatalogRetentionPlan, CapabilityGatewayCatalogRetentionResult,
    CapabilityGatewayCatalogStore, CAPABILITY_GATEWAY_CATALOG_RETENTION_JOURNAL_SCHEMA,
    CAPABILITY_GATEWAY_CATALOG_RETENTION_PLAN_SCHEMA,
    CAPABILITY_GATEWAY_CATALOG_RETENTION_RESULT_SCHEMA, CAPABILITY_GATEWAY_CATALOG_STORE_SCHEMA,
    MAX_CAPABILITY_GATEWAY_CATALOG_BYTES, MAX_CAPABILITY_GATEWAY_CATALOG_RECORDS,
};
#[cfg(feature = "extensions")]
pub use crate::capability_catalog_store::{
    CapabilityGatewayCatalogRestoreEntry, CapabilityGatewayCatalogRestorePlan,
    CapabilityGatewayCatalogRestoreResult, CAPABILITY_GATEWAY_CATALOG_RESTORE_PLAN_SCHEMA,
    CAPABILITY_GATEWAY_CATALOG_RESTORE_RESULT_SCHEMA,
};
pub use admission::CapabilityGatewayLimits;
use admission::{AdmissionFailure, GatewayAdmission};
pub use discovery::{AllowAllCapabilityGatewayDiscoveryPolicy, CapabilityGatewayDiscoveryPolicy};
pub use http::CapabilityGatewayHttpConfig;
pub use notifications::{CapabilityGatewayNotificationHub, CapabilityGatewayNotificationReport};
pub use resolver::{
    CapabilityGatewayInvocation, CapabilityGatewayInvocationFactory,
    CapabilityGatewayInvocationLease, CapabilityGatewayInvocationResolver,
    CapabilityGatewayRegistryResolver, CapabilityGatewayResolvedProvider,
};
pub use session_factory::{
    CapabilityGatewayLiveMcpServer, CapabilityGatewaySessionFactory, CapabilityGatewaySessionKey,
    CapabilityGatewaySessionReplacement,
};

pub use server::CapabilityGatewayMcpServer;

use protocol::{
    discovery_page, mcp_error, validate_prompt_arguments, validate_resource_contents,
    validate_snapshot_binding_identity, CompiledCapabilitySchema, MAX_DISCOVERY_CURSOR_BYTES,
    MCP_AUTHORIZATION_ERROR, MCP_CANCELLED_ERROR, MCP_DISCOVERY_CURSOR_STALE, MCP_DISCOVERY_ERROR,
    MCP_ERROR, MCP_INVOCATION_ERROR, MCP_SCHEMA_ERROR,
};

const MCP_RESOURCE_ERROR: &str = "use.plugin.capability_gateway_resource_failed";
const MCP_PROMPT_ERROR: &str = "use.plugin.capability_gateway_prompt_failed";
const MAX_CAPABILITY_PRINCIPAL_BYTES: usize = 256;
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CapabilityGatewayTransport {
    Stdio,
    StreamableHttp,
}

/// Trusted request context assembled by the Gateway boundary.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
/// Crate-internal lifetime marker for an alternate installation authority.
///
/// The binding check is part of the marker rather than inferred from the
/// catalog alone.  A catalog identity can be copied into an unrelated server;
/// the marker must prove that the retained authority lease selected that same
/// endpoint.  The inactive Control composition implements this marker for its
/// private lease so the MCP layer can retain it without exposing Control
/// storage types through the public API.
pub(crate) trait CapabilityGatewayExternalLease: Send + Sync {
    fn matches_gateway_session(&self, key: &CapabilityGatewaySessionKey) -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CapabilityGatewayGenerationLeaseMode {
    None,
    Registry,
    External,
}

#[cfg(test)]
mod tests;
