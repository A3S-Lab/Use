//! Principal-scoped discovery policy for the Capability Gateway.
//!
//! Discovery is an information boundary, not an authorization grant. A
//! policy can hide a descriptor from a principal's catalog view, while the
//! injected invocation provider remains responsible for authorizing every
//! operation that is actually executed.

use a3s_use_core::{CapabilityDescriptor, UseResult};
use async_trait::async_trait;

use super::CapabilityGatewayRequestContext;

/// Host-owned policy controlling which catalog descriptors are discoverable
/// for one authenticated request context.
///
/// Returning `Ok(false)` removes the descriptor from all standard MCP
/// discovery responses and makes direct access behave as if the descriptor
/// were not published. Returning an error fails the discovery or access
/// request closed with a generic protocol error; the policy's diagnostic is
/// never forwarded to the agent.
///
/// This policy is deliberately separate from
/// [`super::CapabilityGatewayInvocationProvider`](crate::capability_gateway::CapabilityGatewayInvocationProvider):
/// hiding metadata does not authorize an invocation, and an invocation
/// provider must still enforce its principal, scope, Grant, and generation
/// rules for every operation.
#[async_trait]
pub trait CapabilityGatewayDiscoveryPolicy: Send + Sync {
    /// Decide whether one immutable descriptor is visible to the context.
    async fn is_visible(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<bool>;
}

/// Compatibility policy used by the existing constructors.
///
/// Hosts that serve more than one principal should inject an explicit policy
/// with [`super::CapabilityGatewayMcpServer::with_discovery_policy`].
#[derive(Debug, Clone, Copy, Default)]
pub struct AllowAllCapabilityGatewayDiscoveryPolicy;

#[async_trait]
impl CapabilityGatewayDiscoveryPolicy for AllowAllCapabilityGatewayDiscoveryPolicy {
    async fn is_visible(
        &self,
        _descriptor: &CapabilityDescriptor,
        _context: &CapabilityGatewayRequestContext,
    ) -> UseResult<bool> {
        Ok(true)
    }
}
