//! Construction and registry-backed composition for CapabilityGatewayMcpServer.

use std::collections::BTreeMap;
use std::sync::Arc;

#[cfg(feature = "extensions")]
use a3s_use_core::SignedCapabilityDescription;
use a3s_use_core::{
    CapabilityConsumerNegotiation, CapabilityDescriptionProof, CapabilityDescriptor,
    CapabilityGatewayCatalog, UseResult,
};
use tokio::sync::Mutex;

use crate::capability_registry::CapabilitySnapshotLease;

use super::super::admission::{CapabilityGatewayLimits, GatewayAdmission};
use super::super::discovery::AllowAllCapabilityGatewayDiscoveryPolicy;
use super::super::notifications::CapabilityGatewayNotificationHub;
use super::super::protocol::{
    compile_prompts, compile_resources, compile_tools, frozen_tool_router, mcp_error,
    validate_snapshot_binding,
};
use super::super::resolver::{
    CapabilityGatewayInvocationFactory, CapabilityGatewayRegistryResolver,
    CapabilityGatewayResolvedProvider,
};
use super::super::{
    CapabilityGatewayCompositionOptions, CapabilityGatewayExternalLease,
    CapabilityGatewayGenerationLeaseMode, CapabilityGatewayInvocationProvider,
    CapabilityGatewayTransport,
};
use super::CapabilityGatewayMcpServer;

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

    /// Build and bind a Gateway from cryptographically verified signed
    /// descriptions. The trust store is supplied by the host's Registry/TUF
    /// authority; this constructor never accepts a caller-provided signer
    /// assertion as evidence.
    #[cfg(feature = "extensions")]
    pub async fn from_signed_registry_snapshot(
        registry: &crate::capability_registry::CapabilityRegistry,
        signed: Vec<SignedCapabilityDescription>,
        trust_store: &a3s_use_extension::CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    ) -> UseResult<Option<Self>> {
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog_from_signed_descriptions(
            signed,
            trust_store,
            now_unix_seconds,
        )?;
        let Some(lease) = registry.acquire_snapshot_lease(snapshot.cursor()).await? else {
            return Ok(None);
        };
        Self::with_snapshot_lease(catalog, provider, lease).map(Some)
    }

    /// Build and bind a signed-description Gateway for an explicit consumer
    /// negotiation.
    #[cfg(feature = "extensions")]
    pub async fn from_signed_registry_snapshot_with_consumer_negotiation(
        registry: &crate::capability_registry::CapabilityRegistry,
        signed: Vec<SignedCapabilityDescription>,
        trust_store: &a3s_use_extension::CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        negotiation: CapabilityConsumerNegotiation,
    ) -> UseResult<Option<Self>> {
        negotiation.validate()?;
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog_from_signed_descriptions(
            signed,
            trust_store,
            now_unix_seconds,
        )?;
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

    /// Build a Gateway from signed descriptions and a live invocation factory.
    /// Every description crosses the Ed25519 trust boundary before the
    /// Control-bound snapshot and provider lease are acquired.
    #[cfg(feature = "extensions")]
    pub async fn from_signed_registry_snapshot_with_factory(
        registry: &crate::capability_registry::CapabilityRegistry,
        signed: Vec<SignedCapabilityDescription>,
        trust_store: &a3s_use_extension::CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
        factory: Arc<dyn CapabilityGatewayInvocationFactory>,
    ) -> UseResult<Option<Self>> {
        Self::from_signed_registry_snapshot_with_factory_and_options(
            registry,
            signed,
            trust_store,
            now_unix_seconds,
            factory,
            CapabilityGatewayCompositionOptions::default(),
        )
        .await
    }

    /// Build a signed-description Gateway with explicit consumer negotiation
    /// and bounded admission policy.
    #[cfg(feature = "extensions")]
    pub async fn from_signed_registry_snapshot_with_factory_and_options(
        registry: &crate::capability_registry::CapabilityRegistry,
        signed: Vec<SignedCapabilityDescription>,
        trust_store: &a3s_use_extension::CapabilityDescriptionTrustStore,
        now_unix_seconds: u64,
        factory: Arc<dyn CapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<Self>> {
        options.negotiation.validate()?;
        let snapshot = registry.snapshot().await?;
        let catalog = snapshot.capability_gateway_catalog_from_signed_descriptions(
            signed,
            trust_store,
            now_unix_seconds,
        )?;
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
        // Preserve the complete publication identity before projecting the
        // catalog for this consumer. The visible catalog is intentionally a
        // separate immutable view, but leases and cutovers must remain bound
        // to the source publication rather than to that view's derived
        // revision/digest.
        let source_catalog = Arc::new(catalog.clone());
        // Keep the negotiated view as part of the immutable server state. A
        // descriptor requiring an extension that this consumer did not
        // explicitly accept must disappear from both discovery and direct
        // invocation routing.
        let catalog = catalog.for_consumer(&consumer_negotiation)?;
        let catalog_digest = Arc::<str>::from(catalog.descriptor_digest()?);
        let notification_hub = Arc::new(
            CapabilityGatewayNotificationHub::for_catalog(&catalog)
                .map_err(|_| mcp_error("The Capability Gateway notification state is invalid."))?,
        );
        let catalog = Arc::new(catalog);
        let tools = Arc::new(compile_tools(&catalog)?);
        let resources = Arc::new(compile_resources(&catalog)?);
        let prompts = Arc::new(compile_prompts(&catalog)?);
        let tool_router = frozen_tool_router(&catalog)?;
        let admission = Arc::new(GatewayAdmission::new(limits)?);
        Ok(Self {
            source_catalog,
            catalog,
            catalog_digest,
            consumer_negotiation: Arc::new(consumer_negotiation),
            provider,
            tools,
            resources,
            prompts,
            tool_router,
            admission,
            discovery_policy: Arc::new(AllowAllCapabilityGatewayDiscoveryPolicy),
            discovery_policy_snapshot: None,
            discovery_views: Arc::new(Mutex::new(BTreeMap::new())),
            transport: CapabilityGatewayTransport::Stdio,
            snapshot_lease,
            external_lease: None,
            notification_hub,
        })
    }
}
