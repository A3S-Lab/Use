//! Control-owned Capability Gateway session bind, replace, reconcile, and cutover.

#![allow(dead_code)]

use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{CapabilityGatewayCatalog, UseError, UseResult};
use async_trait::async_trait;

use crate::capability_gateway::{
    CapabilityGatewayCompositionOptions, CapabilityGatewayGenerationLeaseMode,
    CapabilityGatewayInvocationProvider, CapabilityGatewayMcpServer,
    CapabilityGatewaySessionFactory, CapabilityGatewaySessionKey,
    CapabilityGatewaySessionReplacement,
};
use crate::plugin_lifecycle::PluginGraphCapabilityCutoverActivation;

use super::super::effect_owner::capability_plane::{
    ControlCapabilityGatewayInvocationFactory, ControlCapabilityGatewayInvocationResolver,
    ControlCapabilityPayloadRetentionResult, ControlCapabilitySnapshotLease,
    LiveControlCapabilityGatewayEndpointRouter, ProductionControlInvocationFactory,
};
use super::super::model::{
    ControlPublishedCapabilityCursor, ControlPublishedCapabilityCutover,
};
use super::{
    ControlStoreRuntimeComposition, CAPABILITY_GATEWAY_DRAIN_BINDING_ERROR,
    CAPABILITY_RETENTION_CURSOR_ERROR,
};


/// Result of reconciling a live Gateway endpoint with the durable Control
/// publication.  An unchanged endpoint is reported separately so recovery
/// does not acquire a second package-generation lease or emit a redundant
/// list-change notification.
#[cfg(feature = "mcp")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::control_store) enum ControlCapabilityGatewayReconciliation {
    Unchanged(CapabilityGatewaySessionKey),
    Replaced(CapabilityGatewaySessionReplacement),
}

/// Lifecycle activation adapter that binds the graph coordinator's
/// post-publication hook to the Control-owned Gateway session factory.
///
/// The factory must already be seeded from Control authority.  Activation
/// then reopens the current durable cursor and swaps the endpoint before the
/// graph coordinator starts draining prior package generations.
#[cfg(feature = "mcp")]
#[derive(Clone)]
pub(in crate::control_store) struct ControlCapabilityGatewayCutoverActivation {
    composition: ControlStoreRuntimeComposition,
    factory: CapabilityGatewaySessionFactory,
    provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    options: CapabilityGatewayCompositionOptions,
}

impl ControlStoreRuntimeComposition {

    /// Drain a live Control-bound Gateway endpoint and then retain exactly the
    /// payloads selected by the durable Control cursor.  This is the shutdown
    /// path for hosts that are retiring an endpoint rather than replacing it:
    /// the session must release its shared generation lease before the
    /// exclusive owner-retention fence can be acquired.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn drain_and_retain_published_capability_gateway(
        &self,
        session: &CapabilityGatewaySessionFactory,
        drain_timeout: Duration,
        additional_catalog_retain_digests: &[String],
        additional_descriptor_snapshot_retain_digests: &[String],
    ) -> UseResult<ControlCapabilityPayloadRetentionResult> {
        // A retention call must drain the endpoint selected by the durable
        // cursor, not merely any session supplied by the host.  Otherwise a
        // stale or unrelated endpoint could be drained while the published
        // generation remains live and its payload is deleted underneath it.
        let binding = self
            .store
            .published_capability_cutover()
            .await?
            .ok_or_else(|| {
                UseError::new(
                    CAPABILITY_GATEWAY_DRAIN_BINDING_ERROR,
                    "The durable Control capability publication is unavailable for Gateway drain.",
                )
            })?;
        let expected_key = gateway_session_key_from_cursor(&binding.cursor)?;
        if !session.drain_if_bound(&expected_key, drain_timeout).await? {
            return Err(UseError::new(
                CAPABILITY_GATEWAY_DRAIN_BINDING_ERROR,
                "The Gateway session does not retain the Control lease for the durable capability publication.",
            ));
        }
        let confirmed = self.store.published_capability_cutover().await?;
        if confirmed.as_ref().map(|value| &value.cursor) != Some(&binding.cursor) {
            return Err(UseError::new(
                CAPABILITY_RETENTION_CURSOR_ERROR,
                "The published Control capability changed while the Gateway session was draining.",
            ));
        }
        let plan = self
            .plan_published_capability_payload_retention(
                additional_catalog_retain_digests,
                additional_descriptor_snapshot_retain_digests,
            )
            .await?;
        let digest = plan.descriptor_digest()?;
        self.apply_published_capability_payload_retention(&plan, &digest)
            .await
    }


    /// Reconstruct a live Gateway endpoint from the durable published Control
    /// cursor after a host restart. The returned session factory retains the
    /// exact Control generation lease in every cloned immutable server.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn reopen_published_capability_gateway(
        &self,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionFactory>> {
        let Some(lease) = self.capability_plane.reopen_published().await? else {
            return Ok(None);
        };
        let server = Self::gateway_server_from_control_lease(lease, provider, options)?;
        Ok(Some(CapabilityGatewaySessionFactory::new(server)))
    }

    /// Reconstruct a live Gateway whose opaque invocation provider is bound to
    /// the same durable Control publication as the session lease. This helper
    /// keeps resolver and endpoint construction together so a host cannot
    /// accidentally pair a Control catalog with a Registry-backed resolver.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn reopen_published_capability_gateway_with_factory(
        &self,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionFactory>> {
        let provider = self.gateway_invocation_provider(factory);
        self.reopen_published_capability_gateway(provider, options)
            .await
    }

    /// Replace an existing live Gateway endpoint from the current durable
    /// Control publication. A missing or raced publication returns `None` so
    /// the host can retry after refreshing its lifecycle view; the factory
    /// itself retains the old server until the new lease-backed server swaps.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn replace_published_capability_gateway(
        &self,
        factory: &CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionReplacement>> {
        let Some(binding) = self.store.published_capability_cutover().await? else {
            return Ok(None);
        };
        // Capture the local source before opening the new Control lease. The
        // final conditional swap below then turns any local cutover race into
        // a retry result instead of allowing this build to roll the endpoint
        // back to an older same-generation projection.
        let expected_current = factory.current_key()?;
        if expected_current.generation > binding.cursor.capability_generation {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_stale",
                "The durable Control publication is older than the live Gateway endpoint.",
            ));
        }
        let Some(lease) = self
            .capability_plane
            .acquire_published(&binding.cursor)
            .await?
        else {
            return Ok(None);
        };
        let server = Self::gateway_server_from_control_lease(lease, provider, options)?;
        let next = gateway_session_key(server.source_catalog())?;
        let Some(confirmed) = self.store.published_capability_cutover().await? else {
            return Ok(None);
        };
        if confirmed.cursor != binding.cursor {
            return Ok(None);
        }
        let current = factory.current_key()?;
        if current.generation > next.generation {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_stale",
                "The durable Control publication is older than the live Gateway endpoint.",
            ));
        }
        let Some(replacement) = factory
            .replace_if_current(&expected_current, server)
            .await?
        else {
            return Ok(None);
        };
        Ok(Some(replacement))
    }

    /// Replace a live Gateway from the current durable Control publication
    /// while constructing its resolver from the same Control authority.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn replace_published_capability_gateway_with_factory(
        &self,
        session: &CapabilityGatewaySessionFactory,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<CapabilityGatewaySessionReplacement>> {
        let provider = self.gateway_invocation_provider(factory);
        self.replace_published_capability_gateway(session, provider, options)
            .await
    }

    /// Reconcile a live Control-bound Gateway endpoint with the durable
    /// publication, without replacing an endpoint that already serves the
    /// exact same immutable catalog.  A newer in-memory endpoint is rejected
    /// rather than silently moving the durable authority backwards; callers
    /// can retry after refreshing their Control view.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) async fn reconcile_published_capability_gateway(
        &self,
        factory: &CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<Option<ControlCapabilityGatewayReconciliation>> {
        let Some(binding) = self.store.published_capability_cutover().await? else {
            return Ok(None);
        };
        self.reconcile_published_capability_gateway_binding(factory, provider, options, &binding)
            .await
    }

    /// Reconcile against one already-coherent cursor/operation read.  The
    /// exact cursor is reacquired below rather than reopening whatever happens
    /// to be current, so a concurrent publication can only make this attempt
    /// return `None` and force the lifecycle caller to retry.
    #[cfg(feature = "mcp")]
    async fn reconcile_published_capability_gateway_binding(
        &self,
        factory: &CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
        binding: &ControlPublishedCapabilityCutover,
    ) -> UseResult<Option<ControlCapabilityGatewayReconciliation>> {
        let expected = gateway_session_key_from_cursor(&binding.cursor)?;
        let current = factory.current_key()?;
        let current_is_bound = {
            let current_server = factory.current();
            current_server.generation_lease_mode() == CapabilityGatewayGenerationLeaseMode::External
                // During an upgrade the live endpoint legitimately retains
                // the previous Control lease until this reconciliation swaps
                // in the newly published server. Validate that lease against
                // the endpoint it actually serves; comparing it with the
                // target publication would reject every normal cutover.
                && current_server.external_lease_matches(&current)
        };
        if !current_is_bound {
            return Err(UseError::new(
                "use.control.capability_gateway_activation_invalid",
                "The live Gateway endpoint is not bound to the Control generation lease authority.",
            ));
        }
        if current == expected {
            return Ok(Some(ControlCapabilityGatewayReconciliation::Unchanged(
                current,
            )));
        }
        if current.generation > expected.generation {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_stale",
                "The durable Control publication is older than the live Gateway endpoint.",
            ));
        }

        let Some(lease) = self
            .capability_plane
            .acquire_published(&binding.cursor)
            .await?
        else {
            return Ok(None);
        };
        let server = Self::gateway_server_from_control_lease(lease, provider, options)?;
        // Session identity follows the complete Control publication retained
        // by the server, while `server.catalog()` is only the negotiated
        // consumer view and may omit optional descriptors.
        let next = gateway_session_key(server.source_catalog())?;
        // The lease admission above proves the cursor was still exact while
        // all package-generation locks and payload bytes were acquired.  A
        // final authority read prevents swapping a freshly built endpoint if
        // another publication completed while the provider server was being
        // compiled.
        let Some(confirmed) = self.store.published_capability_cutover().await? else {
            return Ok(None);
        };
        if confirmed.cursor != binding.cursor {
            return Ok(None);
        }
        let current = factory.current_key()?;
        if current == next {
            return Ok(Some(ControlCapabilityGatewayReconciliation::Unchanged(
                current,
            )));
        }
        if current.generation > next.generation {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_stale",
                "The durable Control publication is older than the live Gateway endpoint.",
            ));
        }
        let Some(replacement) = factory.replace_if_current(&current, server).await? else {
            // Another local cutover changed the source after the final
            // durable cursor read.  Do not overwrite it with the server built
            // from this attempt, especially when both publications share a
            // generation but have different revisions.  The lifecycle caller
            // can reopen Control authority and retry from a coherent view.
            return Ok(None);
        };
        Ok(Some(ControlCapabilityGatewayReconciliation::Replaced(
            replacement,
        )))
    }

    /// Build the graph-lifecycle activation adapter for a Gateway factory
    /// that was seeded from this composition's Control authority.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_cutover_activation(
        &self,
        factory: CapabilityGatewaySessionFactory,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> Arc<dyn PluginGraphCapabilityCutoverActivation> {
        Arc::new(ControlCapabilityGatewayCutoverActivation {
            composition: self.clone(),
            factory,
            provider,
            options,
        })
    }

    /// Build a lifecycle activation hook whose resolver and session factory
    /// are both derived from this Control composition. Keeping this helper at
    /// the boundary prevents a host from attaching a Registry-backed provider
    /// to a Control-leased endpoint by accident.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_cutover_activation_with_factory(
        &self,
        session: CapabilityGatewaySessionFactory,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
        options: CapabilityGatewayCompositionOptions,
    ) -> Arc<dyn PluginGraphCapabilityCutoverActivation> {
        let provider = self.gateway_invocation_provider(factory);
        self.gateway_cutover_activation(session, provider, options)
    }

    /// Build the host provider that resolves opaque references through this
    /// composition's exact Control publication and generation lease.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_invocation_provider(
        &self,
        factory: Arc<dyn ControlCapabilityGatewayInvocationFactory>,
    ) -> Arc<dyn CapabilityGatewayInvocationProvider> {
        Arc::new(
            crate::capability_gateway::CapabilityGatewayResolvedProvider::new(Arc::new(
                ControlCapabilityGatewayInvocationResolver::new(
                    Arc::clone(&self.capability_plane),
                    factory,
                ),
            )),
        )
    }

    /// Production provider: Control Grant + Runtime receipt/provider join at
    /// open. Tool Task invoke uses the joined receipt; Tool/MCP Service invoke
    /// proves the provider lease then routes through the composition-owned
    /// live Gateway endpoint table recorded at bind readiness.
    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn production_gateway_invocation_provider(
        &self,
    ) -> Arc<dyn CapabilityGatewayInvocationProvider> {
        self.gateway_invocation_provider(Arc::new(
            ProductionControlInvocationFactory::with_endpoint_router(
                self.store.clone(),
                Arc::clone(&self.runtime_resolver),
                self.runtime_bindings.clone(),
                self.plan_store.clone(),
                Arc::new(LiveControlCapabilityGatewayEndpointRouter::new(Arc::clone(
                    &self.endpoint_routes,
                ))),
            ),
        ))
    }

    #[cfg(feature = "mcp")]
    pub(in crate::control_store) fn gateway_server_from_control_lease(
        lease: ControlCapabilitySnapshotLease,
        provider: Arc<dyn CapabilityGatewayInvocationProvider>,
        options: CapabilityGatewayCompositionOptions,
    ) -> UseResult<CapabilityGatewayMcpServer> {
        let CapabilityGatewayCompositionOptions {
            negotiation,
            limits,
        } = options;
        let catalog = lease.catalog().clone();
        let projected = catalog.for_consumer(&negotiation)?;
        lease.validate_gateway_catalog(&projected)?;
        let lease: Arc<dyn crate::capability_gateway::CapabilityGatewayExternalLease> =
            Arc::new(lease);
        let server = CapabilityGatewayMcpServer::with_consumer_negotiation_and_limits(
            catalog,
            provider,
            negotiation,
            limits,
        )?;
        server.with_external_lease(lease)
    }
}


#[cfg(feature = "mcp")]
#[async_trait]
impl PluginGraphCapabilityCutoverActivation for ControlCapabilityGatewayCutoverActivation {
    async fn activate_capability_cutover(&self, idempotency_key: &str) -> UseResult<()> {
        // The graph callback carries an opaque key derived from the reviewed
        // package plan.  Bind it to the exact durable Control operation that
        // owns the published cursor before reconciling the live endpoint;
        // otherwise a stale replay could accidentally activate a newer
        // publication simply because one exists.
        let Some(binding) = self
            .composition
            .store
            .published_capability_cutover()
            .await?
        else {
            return Err(UseError::new(
                "use.control.capability_gateway_publication_missing",
                "The lifecycle cutover has no durable Control Gateway publication to activate.",
            ));
        };
        let Some(expected_key) = binding.graph_cutover_key.as_deref() else {
            return Err(UseError::new(
                "use.control.capability_gateway_activation_key_mismatch",
                "The durable Control capability publication is not owned by a package-graph cutover.",
            ));
        };
        if expected_key != idempotency_key {
            return Err(UseError::new(
                "use.control.capability_gateway_activation_key_mismatch",
                "The lifecycle cutover key does not match the durable Control capability publication.",
            ));
        }
        self.composition
            .reconcile_published_capability_gateway_binding(
                &self.factory,
                Arc::clone(&self.provider),
                self.options.clone(),
                &binding,
            )
            .await?
            .ok_or_else(|| {
                UseError::new(
                    "use.control.capability_gateway_publication_missing",
                    "The lifecycle cutover has no durable Control Gateway publication to activate.",
                )
            })?;
        Ok(())
    }
}


#[cfg(feature = "mcp")]
fn gateway_session_key_from_cursor(
    cursor: &ControlPublishedCapabilityCursor,
) -> UseResult<CapabilityGatewaySessionKey> {
    cursor.validate()?;
    Ok(CapabilityGatewaySessionKey {
        installation: cursor.installation.clone(),
        generation: cursor.catalog.generation,
        revision: cursor.catalog.revision.clone(),
        digest: cursor.catalog.digest.clone(),
    })
}

#[cfg(feature = "mcp")]
fn gateway_session_key(
    catalog: &CapabilityGatewayCatalog,
) -> UseResult<CapabilityGatewaySessionKey> {
    catalog.validate()?;
    Ok(CapabilityGatewaySessionKey {
        installation: catalog.installation().clone(),
        generation: catalog.generation(),
        revision: catalog.revision().to_owned(),
        digest: catalog.descriptor_digest()?,
    })
}
