//! Host-owned live invocation resolution for the Capability Gateway.
//!
//! The MCP adapter only knows an opaque [`InvocationRef`]. This module gives
//! embedding hosts an explicit seam for resolving that reference to a private
//! handle. The returned handle is a lease: its implementation must own the
//! exact package-generation guard and keep it alive until `invoke` returns.

use std::fmt;
use std::sync::Arc;

use a3s_use_core::{CapabilityDescriptor, InvocationRef, UseError, UseResult};
use async_trait::async_trait;
use serde_json::Value;

use super::{
    CapabilityGatewayInvocationFailure, CapabilityGatewayInvocationProvider,
    CapabilityGatewayRequestContext, CapabilitySnapshotCursor, CapabilitySnapshotLease,
};
use crate::capability_registry::CapabilityRegistry;

/// A private, generation-fenced invocation handle.
///
/// Implementations normally capture an RAII package-generation lease when the
/// handle is created. The handle must not expose paths, credentials, or other
/// host-owned binding details to the caller.
#[async_trait]
pub trait CapabilityGatewayInvocation: Send + Sync {
    /// Apply the host's principal and Grant policy to the validated arguments.
    async fn authorize(
        &self,
        arguments: &Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<()>;

    /// Execute the already-authorized invocation while retaining this handle's
    /// generation lease for the complete operation and returned value.
    async fn invoke(
        &self,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value>;
}

/// A resolved invocation together with the opaque identity it was resolved
/// for. The inner handle owns the actual lifecycle lease.
pub struct CapabilityGatewayInvocationLease {
    invocation_ref: InvocationRef,
    handle: Box<dyn CapabilityGatewayInvocation>,
    /// Optional standard Use snapshot lease. Custom handles may carry an
    /// additional Runtime or provider lease internally; this field makes the
    /// common package-generation guard explicit and keeps it alive for the
    /// complete call.
    snapshot_lease: Option<Arc<CapabilitySnapshotLease>>,
}

impl CapabilityGatewayInvocationLease {
    /// Bind a private invocation handle to the reference returned in the
    /// immutable catalog.
    pub fn new(
        invocation_ref: InvocationRef,
        handle: Box<dyn CapabilityGatewayInvocation>,
    ) -> Self {
        Self {
            invocation_ref,
            handle,
            snapshot_lease: None,
        }
    }

    /// Bind the invocation handle to an exact Use snapshot lease. The lease is
    /// retained until this value is dropped, which must be after invocation
    /// completion.
    pub fn with_snapshot_lease(
        invocation_ref: InvocationRef,
        snapshot_lease: CapabilitySnapshotLease,
        handle: Box<dyn CapabilityGatewayInvocation>,
    ) -> Self {
        Self {
            invocation_ref,
            handle,
            snapshot_lease: Some(Arc::new(snapshot_lease)),
        }
    }

    /// Return the reference this lease was resolved for.
    pub fn invocation_ref(&self) -> &InvocationRef {
        &self.invocation_ref
    }

    /// Return the exact snapshot cursor held by this invocation, when the
    /// host supplied the standard Use lease in [`Self::with_snapshot_lease`].
    pub fn snapshot_cursor(&self) -> Option<&super::CapabilitySnapshotCursor> {
        self.snapshot_lease
            .as_deref()
            .map(CapabilitySnapshotLease::cursor)
    }

    pub(crate) async fn authorize(
        &self,
        arguments: &Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<()> {
        self.handle.authorize(arguments, context).await
    }

    pub(crate) async fn invoke(
        &self,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value> {
        self.handle.invoke(arguments, context).await
    }
}

impl fmt::Debug for CapabilityGatewayInvocationLease {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayInvocationLease")
            .field("invocation_ref", &self.invocation_ref)
            .field("has_snapshot_lease", &self.snapshot_lease.is_some())
            .finish_non_exhaustive()
    }
}

/// Host authority that resolves an immutable descriptor to one live
/// invocation lease. Implementations must compare the descriptor's complete
/// package, surface, generation, and digest identity before opening any
/// provider resource. A missing, stale, or cross-scope binding must fail
/// closed.
#[async_trait]
pub trait CapabilityGatewayInvocationResolver: Send + Sync {
    async fn resolve(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<CapabilityGatewayInvocationLease>;
}

/// Host factory for one private invocation handle. The exact Use snapshot
/// lease is supplied to the factory so a Runtime/receipt resolver can inspect
/// the same generation evidence that the Gateway pins for the call.
#[async_trait]
pub trait CapabilityGatewayInvocationFactory: Send + Sync {
    async fn open(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
        snapshot: &CapabilitySnapshotLease,
    ) -> UseResult<Box<dyn CapabilityGatewayInvocation>>;
}

/// Concrete resolver for a host-owned [`CapabilityRegistry`] publication.
///
/// It captures one immutable cursor, acquires that exact snapshot before each
/// call, and wraps the factory handle with the acquired RAII lease. A changed
/// publication or draining package therefore fails before the factory can
/// open provider state.
#[derive(Clone)]
pub struct CapabilityGatewayRegistryResolver {
    registry: CapabilityRegistry,
    cursor: CapabilitySnapshotCursor,
    factory: Arc<dyn CapabilityGatewayInvocationFactory>,
}

impl CapabilityGatewayRegistryResolver {
    /// Capture an already observed immutable publication cursor.
    pub fn new(
        registry: CapabilityRegistry,
        cursor: CapabilitySnapshotCursor,
        factory: Arc<dyn CapabilityGatewayInvocationFactory>,
    ) -> UseResult<Self> {
        cursor.validate()?;
        if cursor.installation != *registry.installation() {
            return Err(UseError::new(
                "use.capability.snapshot_scope_mismatch",
                "The Gateway resolver cursor belongs to a different installation.",
            ));
        }
        Ok(Self {
            registry,
            cursor,
            factory,
        })
    }

    /// Observe and capture the Registry's current immutable publication.
    pub async fn from_current(
        registry: CapabilityRegistry,
        factory: Arc<dyn CapabilityGatewayInvocationFactory>,
    ) -> UseResult<Self> {
        let cursor = registry.snapshot().await?.cursor().clone();
        Self::new(registry, cursor, factory)
    }

    pub fn cursor(&self) -> &CapabilitySnapshotCursor {
        &self.cursor
    }
}

impl fmt::Debug for CapabilityGatewayRegistryResolver {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayRegistryResolver")
            .field("installation", self.registry.installation())
            .field("cursor", &self.cursor)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CapabilityGatewayInvocationResolver for CapabilityGatewayRegistryResolver {
    async fn resolve(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<CapabilityGatewayInvocationLease> {
        let package = self
            .cursor
            .packages
            .iter()
            .find(|package| package.package_id == descriptor.package_id.to_string())
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.capability_gateway_resolution_mismatch",
                    "The capability is outside the captured Use publication.",
                )
            })?;
        if package.lifecycle_generation != descriptor.generation
            || package.package_digest != descriptor.package_digest
            || package.manifest_digest != descriptor.manifest_digest
        {
            return Err(UseError::new(
                "use.plugin.capability_gateway_resolution_mismatch",
                "The capability does not match the captured package generation.",
            ));
        }

        let snapshot = self
            .registry
            .acquire_snapshot_lease(&self.cursor)
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.plugin.capability_gateway_snapshot_unavailable",
                    "The captured capability publication is no longer callable.",
                )
            })?;
        let handle = self.factory.open(descriptor, context, &snapshot).await?;
        Ok(CapabilityGatewayInvocationLease::with_snapshot_lease(
            descriptor.invocation_ref.clone(),
            snapshot,
            handle,
        ))
    }
}

/// Adapter that turns a host resolver into the Gateway's invocation-provider
/// contract. It performs one resolution and one authorization for each live
/// call, then drops the lease only after the invocation result is complete.
#[derive(Clone)]
pub struct CapabilityGatewayResolvedProvider {
    resolver: Arc<dyn CapabilityGatewayInvocationResolver>,
}

impl CapabilityGatewayResolvedProvider {
    pub fn new(resolver: Arc<dyn CapabilityGatewayInvocationResolver>) -> Self {
        Self { resolver }
    }

    pub fn resolver(&self) -> &dyn CapabilityGatewayInvocationResolver {
        self.resolver.as_ref()
    }

    async fn resolve(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<CapabilityGatewayInvocationLease> {
        let lease = self.resolver.resolve(descriptor, context).await?;
        if lease.invocation_ref() != &descriptor.invocation_ref {
            return Err(UseError::new(
                "use.plugin.capability_gateway_resolution_mismatch",
                "The resolved invocation does not match the published opaque reference.",
            ));
        }
        if let Some(cursor) = lease.snapshot_cursor() {
            cursor.validate().map_err(|_| {
                UseError::new(
                    "use.plugin.capability_gateway_resolution_mismatch",
                    "The resolved invocation carries an invalid snapshot lease.",
                )
            })?;
            let Some(package) = cursor
                .packages
                .iter()
                .find(|package| package.package_id == descriptor.package_id.to_string())
            else {
                return Err(UseError::new(
                    "use.plugin.capability_gateway_resolution_mismatch",
                    "The resolved invocation is outside the published snapshot.",
                ));
            };
            if package.lifecycle_generation != descriptor.generation
                || package.package_digest != descriptor.package_digest
                || package.manifest_digest != descriptor.manifest_digest
            {
                return Err(UseError::new(
                    "use.plugin.capability_gateway_resolution_mismatch",
                    "The resolved invocation does not match the published package generation.",
                ));
            }
        }
        Ok(lease)
    }
}

impl fmt::Debug for CapabilityGatewayResolvedProvider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayResolvedProvider")
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl CapabilityGatewayInvocationProvider for CapabilityGatewayResolvedProvider {
    async fn authorize(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: &Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<()> {
        let lease = self.resolve(descriptor, context).await?;
        lease.authorize(arguments, context).await
    }

    async fn invoke(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<Value> {
        let lease = self.resolve(descriptor, context).await?;
        // Keep the provider safe when it is called directly, outside the MCP
        // adapter's normal authorize-then-invoke sequence.
        lease.authorize(&arguments, context).await?;
        lease.invoke(arguments, context).await
    }

    async fn authorize_and_invoke(
        &self,
        descriptor: &CapabilityDescriptor,
        arguments: Value,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<Value, CapabilityGatewayInvocationFailure> {
        let lease = self
            .resolve(descriptor, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Invocation)?;
        lease
            .authorize(&arguments, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Authorization)?;
        lease
            .invoke(arguments, context)
            .await
            .map_err(CapabilityGatewayInvocationFailure::Invocation)
    }
}
