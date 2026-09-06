//! Control-backed opaque invocation resolution.
//!
//! The public Gateway only carries an opaque [`a3s_use_core::InvocationRef`].
//! This adapter
//! is the missing authority join for the Control Store: it reopens the
//! durable published cursor, validates the complete descriptor against the
//! immutable catalog, and gives a host-owned factory the exact leased
//! generation before any provider state is opened.

use std::sync::Arc;

use a3s_use_core::{CapabilityDescriptor, UseError, UseResult};
use async_trait::async_trait;

use crate::capability_gateway::{
    CapabilityGatewayExternalLease, CapabilityGatewayInvocation, CapabilityGatewayInvocationLease,
    CapabilityGatewayInvocationResolver, CapabilityGatewayRequestContext,
};

use super::{ControlCapabilityPlaneEffectPort, ControlCapabilitySnapshotLease};

const RESOLUTION_ERROR: &str = "use.control.capability_gateway_resolution_unavailable";

/// Host-owned factory for one Control-bound opaque invocation.
///
/// The factory receives the exact immutable Control lease that was validated
/// against the published catalog. It may use that authority to join the
/// descriptor to a Grant, Runtime receipt, provider process, or other private
/// binding, but must not expose those details to the Gateway client.
#[async_trait]
pub(in crate::control_store) trait ControlCapabilityGatewayInvocationFactory:
    Send + Sync
{
    async fn open(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
        lease: &ControlCapabilitySnapshotLease,
    ) -> UseResult<Box<dyn CapabilityGatewayInvocation>>;
}

/// Resolver that joins an opaque descriptor to the current durable Control
/// publication and retains its generation lease through the complete call.
#[derive(Clone)]
pub(in crate::control_store) struct ControlCapabilityGatewayInvocationResolver {
    plane: Arc<ControlCapabilityPlaneEffectPort>,
    factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
}

impl ControlCapabilityGatewayInvocationResolver {
    pub(in crate::control_store) fn new(
        plane: Arc<ControlCapabilityPlaneEffectPort>,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
    ) -> Self {
        Self { plane, factory }
    }
}

#[async_trait]
impl CapabilityGatewayInvocationResolver for ControlCapabilityGatewayInvocationResolver {
    async fn resolve(
        &self,
        descriptor: &CapabilityDescriptor,
        context: &CapabilityGatewayRequestContext,
    ) -> UseResult<CapabilityGatewayInvocationLease> {
        let lease = self.plane.reopen_published().await?.ok_or_else(|| {
            UseError::new(
                RESOLUTION_ERROR,
                "The published Control capability generation is unavailable for invocation.",
            )
        })?;
        lease.validate_gateway_descriptor(descriptor)?;
        let handle = self.factory.open(descriptor, context, &lease).await?;
        let external_lease: Arc<dyn CapabilityGatewayExternalLease> = Arc::new(lease);
        Ok(CapabilityGatewayInvocationLease::with_external_lease(
            descriptor.invocation_ref.clone(),
            external_lease,
            handle,
        ))
    }
}
