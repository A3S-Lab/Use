//! Standard MCP server implementation for the Capability Gateway.

use std::collections::BTreeMap;
use std::sync::Arc;

use a3s_use_core::{
    CapabilityConsumerNegotiation, CapabilityConsumerProfile, CapabilityGatewayCatalog, UseResult,
};
use rmcp::handler::server::router::tool::ToolRouter;
use rmcp::model::{CallToolResult, JsonObject};
use rmcp::ServiceExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

use crate::capability_registry::{CapabilitySnapshotCursor, CapabilitySnapshotLease};

use super::admission::{AdmissionFailure, CapabilityGatewayLimits, GatewayAdmission};
use super::discovery::CapabilityGatewayDiscoveryPolicy;
use super::notifications::{CapabilityGatewayNotificationHub, CapabilityGatewayNotificationReport};
use super::protocol::{
    cancellation_error, descriptor_is_visible, discovery_policy_error, mcp_error,
    run_until_cancelled, structured_error, tool_result, update_discovery_digest_field,
    CapabilityGatewayPrompt, CapabilityGatewayResource, CapabilityGatewayTool,
    MCP_AUTHORIZATION_ERROR, MCP_CANCELLED_ERROR, MCP_RATE_LIMIT_ERROR,
};
use super::{
    CapabilityGatewayExternalLease, CapabilityGatewayGenerationLeaseMode,
    CapabilityGatewayInvocationFailure, CapabilityGatewayInvocationProvider,
    CapabilityGatewayRequestContext, CapabilityGatewaySessionKey, CapabilityGatewayTransport,
};

const MAX_DISCOVERY_CONTEXTS: usize = 64;

type DiscoveryView = Arc<[usize]>;
type DiscoveryViewCell = Arc<OnceCell<DiscoveryView>>;
type DiscoveryViewCache = Arc<Mutex<BTreeMap<CapabilityGatewayRequestContext, DiscoveryViewCell>>>;

#[derive(Clone)]
pub struct CapabilityGatewayMcpServer {
    /// Immutable source publication before consumer negotiation projects the
    /// visible descriptor view. Session lifecycle identity must follow this
    /// source, because a negotiated subset is a presentation concern and can
    /// differ between otherwise identical Control-bound endpoints.
    source_catalog: Arc<CapabilityGatewayCatalog>,
    catalog: Arc<CapabilityGatewayCatalog>,
    /// Digest of the visible catalog projection. Discovery cursors bind to
    /// this value so a replacement cannot reinterpret an old offset.
    catalog_digest: Arc<str>,
    consumer_negotiation: Arc<CapabilityConsumerNegotiation>,
    provider: Arc<dyn CapabilityGatewayInvocationProvider>,
    pub(crate) tools: Arc<BTreeMap<String, CapabilityGatewayTool>>,
    resources: Arc<BTreeMap<String, CapabilityGatewayResource>>,
    prompts: Arc<BTreeMap<String, CapabilityGatewayPrompt>>,
    pub(crate) tool_router: ToolRouter<Self>,
    admission: Arc<GatewayAdmission>,
    discovery_policy: Arc<dyn CapabilityGatewayDiscoveryPolicy>,
    /// Identity of the immutable discovery-policy snapshot. `None` denotes
    /// the built-in allow-all policy; a token is assigned whenever a host
    /// installs a custom policy so lifecycle cutovers can detect a changed
    /// discovery contract even when the source catalog is unchanged.
    discovery_policy_snapshot: Option<Arc<()>>,
    /// One immutable visibility view is retained per trusted request context.
    /// `OnceCell` prevents concurrent requests from observing different
    /// policy decisions for the same pagination cursor.
    discovery_views: DiscoveryViewCache,
    transport: CapabilityGatewayTransport,
    /// When present, this RAII lease pins every callable package generation
    /// for the lifetime of the MCP service (including cloned session handles).
    snapshot_lease: Option<Arc<CapabilitySnapshotLease>>,
    /// Internal composition hook for an installation authority other than the
    /// legacy Capability Registry. Retaining this value makes the authority's
    /// generation lease follow every cloned Gateway server.
    external_lease: Option<Arc<dyn CapabilityGatewayExternalLease>>,
    /// Shared standard MCP list-change fan-out. The catalog itself remains
    /// immutable; a host may use this hub while replacing the session factory
    /// with a newer generation-bound server.
    notification_hub: Arc<CapabilityGatewayNotificationHub>,
}

impl std::fmt::Debug for CapabilityGatewayMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayMcpServer")
            .field("catalog", &self.catalog)
            .field("consumer_negotiation", &self.consumer_negotiation)
            .field("discovery_context_count", &self.discovery_views_count())
            .field("has_snapshot_lease", &self.snapshot_lease.is_some())
            .field("has_external_lease", &self.external_lease.is_some())
            .field("notification_hub", &self.notification_hub)
            .field("transport", &self.transport)
            .finish_non_exhaustive()
    }
}

mod compose;
mod handler;

impl CapabilityGatewayMcpServer {
    pub fn catalog(&self) -> &CapabilityGatewayCatalog {
        &self.catalog
    }

    /// Return the complete immutable publication from which the consumer
    /// visible catalog was projected. This is crate-internal because hosts
    /// should use [`Self::catalog`] for discovery and invocation; lifecycle
    /// code uses this source identity to bind leases and cutovers.
    pub(crate) fn source_catalog(&self) -> &CapabilityGatewayCatalog {
        &self.source_catalog
    }

    /// Return the immutable consumer negotiation bound to this Gateway.
    pub fn consumer_negotiation(&self) -> &CapabilityConsumerNegotiation {
        &self.consumer_negotiation
    }

    /// Return the negotiated consumer profile bound to this Gateway.
    pub fn consumer_profile(&self) -> &CapabilityConsumerProfile {
        self.consumer_negotiation.profile()
    }

    /// Return descriptors that carry Flow/Knowledge/UI metadata for the
    /// negotiated consumer. Generic MCP consumers see an empty list.
    pub fn extension_metadata_descriptors(&self) -> Vec<&a3s_use_core::CapabilityDescriptor> {
        self.catalog
            .descriptors()
            .iter()
            .filter(|descriptor| descriptor.is_extension_metadata())
            .collect()
    }

    /// Attach a host-owned principal discovery policy.
    ///
    /// The policy is evaluated lazily for each distinct trusted request
    /// context and its descriptor decisions are frozen for the lifetime of
    /// the returned server. This keeps MCP pagination views stable even when
    /// requests are concurrent. Existing constructors remain
    /// backwards-compatible with an allow-all policy.
    pub fn with_discovery_policy(
        mut self,
        policy: Arc<dyn CapabilityGatewayDiscoveryPolicy>,
    ) -> Self {
        self.discovery_policy = policy;
        self.discovery_policy_snapshot = Some(Arc::new(()));
        self.discovery_views = Arc::new(Mutex::new(BTreeMap::new()));
        self
    }

    /// Return whether two servers retain the same immutable discovery-policy
    /// snapshot. This is intentionally identity-based: policy implementations
    /// are host-owned and need not expose a stable or hashable configuration,
    /// while replacing a snapshot must conservatively trigger a fresh list
    /// view for MCP clients.
    pub(crate) fn same_discovery_policy_snapshot(&self, other: &Self) -> bool {
        match (
            &self.discovery_policy_snapshot,
            &other.discovery_policy_snapshot,
        ) {
            (None, None) => true,
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }

    pub(crate) fn discovery_policy_snapshot(&self) -> Option<Arc<()>> {
        self.discovery_policy_snapshot.clone()
    }

    fn discovery_views_count(&self) -> usize {
        self.discovery_views
            .try_lock()
            .map(|views| views.len())
            .unwrap_or_default()
    }

    /// Derive the opaque identity carried by a discovery cursor. The digest
    /// covers the complete visible catalog projection and the frozen policy
    /// view, so a cursor from another publication, consumer surface, or
    /// principal view cannot be interpreted as a bare offset.
    fn discovery_cursor_fingerprint(&self, surface: &str, view: &[usize]) -> String {
        let mut hasher = Sha256::new();
        update_discovery_digest_field(&mut hasher, "a3s.use.capability-gateway-cursor.v2");
        update_discovery_digest_field(&mut hasher, surface);
        update_discovery_digest_field(&mut hasher, &self.catalog_digest);
        for index in view {
            // Encode indices at a fixed width so a cursor remains portable
            // when an installation is reconstructed on a different host
            // architecture.
            hasher.update((*index as u64).to_be_bytes());
        }
        format!("{:x}", hasher.finalize())
    }

    /// Return the exact lease cursor when this server is bound to a live Use
    /// snapshot. A contract-only server returns `None`.
    pub fn snapshot_cursor(&self) -> Option<&CapabilitySnapshotCursor> {
        self.snapshot_lease
            .as_deref()
            .map(CapabilitySnapshotLease::cursor)
    }

    /// Attach an installation-owned generation lease from an internal
    /// authority. The caller must validate that the server catalog is the
    /// exact projection of this lease before invoking the hook.
    pub(crate) fn with_external_lease(
        mut self,
        lease: Arc<dyn CapabilityGatewayExternalLease>,
    ) -> UseResult<Self> {
        if self.snapshot_lease.is_some() || self.external_lease.is_some() {
            return Err(mcp_error(
                "The Capability Gateway already retains a generation lease authority.",
            ));
        }
        self.external_lease = Some(lease);
        Ok(self)
    }

    /// Remove the generation lease from a server that is no longer admitted
    /// by its owning session factory.  This is intentionally crate-private:
    /// an unleased server is suitable only as a drained diagnostic snapshot,
    /// never as a live invocation endpoint.
    pub(crate) fn without_generation_lease(mut self) -> Self {
        self.snapshot_lease = None;
        self.external_lease = None;
        self
    }

    /// Return whether this server retains any complete generation lease.
    /// Session replacement uses this to prevent an accidentally unleased
    /// server from replacing a leased endpoint (or vice versa).
    #[cfg(test)]
    pub(crate) fn has_generation_lease(&self) -> bool {
        self.generation_lease_mode() != CapabilityGatewayGenerationLeaseMode::None
    }

    /// Keep one live endpoint on a single generation-authority class. A
    /// Registry lease and a Control lease are both safe individually, but
    /// silently switching between them would change lifecycle authority.
    pub(crate) fn generation_lease_mode(&self) -> CapabilityGatewayGenerationLeaseMode {
        if self.snapshot_lease.is_some() {
            CapabilityGatewayGenerationLeaseMode::Registry
        } else if self.external_lease.is_some() {
            CapabilityGatewayGenerationLeaseMode::External
        } else {
            CapabilityGatewayGenerationLeaseMode::None
        }
    }

    /// Prove that this server's erased external authority lease selected the
    /// exact immutable endpoint identity.  The check deliberately returns
    /// `false` for a server without an external lease; matching catalog bytes
    /// alone are not lifecycle authority.
    pub(crate) fn external_lease_matches(&self, key: &CapabilityGatewaySessionKey) -> bool {
        self.external_lease
            .as_ref()
            .is_some_and(|lease| lease.matches_gateway_session(key))
    }

    /// Return the shared standard MCP list-change hub for this server.
    ///
    /// A production host should share this value with the component that
    /// creates the next immutable, generation-bound session server. Calling
    /// [`CapabilityGatewayNotificationHub::notify_catalog_changed`] after the
    /// new catalog is durably published prompts connected clients to re-list
    /// without inventing an A3S-specific wire method.
    pub fn notification_hub(&self) -> Arc<CapabilityGatewayNotificationHub> {
        Arc::clone(&self.notification_hub)
    }

    /// Reuse a host-owned notification hub when constructing the next
    /// generation-bound server. The hub and catalog must belong to the same
    /// installation; the hub's monotonic publication key intentionally may be
    /// ahead of this server while an older session drains.
    pub fn with_notification_hub(
        mut self,
        notification_hub: Arc<CapabilityGatewayNotificationHub>,
    ) -> UseResult<Self> {
        if notification_hub.installation() != self.catalog.installation() {
            return Err(mcp_error(
                "The Capability Gateway notification hub belongs to another installation.",
            ));
        }
        self.notification_hub = notification_hub;
        Ok(self)
    }

    /// Broadcast a newer catalog through the server's shared notification
    /// hub. The server's own catalog is intentionally not mutated; callers
    /// must route future sessions to the supplied immutable catalog.
    pub async fn notify_catalog_changed(
        &self,
        catalog: &CapabilityGatewayCatalog,
    ) -> UseResult<CapabilityGatewayNotificationReport> {
        self.notification_hub.notify_catalog_changed(catalog).await
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

    pub(crate) fn request_context(
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

    #[cfg(test)]
    pub(crate) async fn discovery_view(
        &self,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<Arc<[usize]>, rmcp::ErrorData> {
        self.discovery_view_with_cancellation(context, &CancellationToken::new())
            .await
    }

    async fn discovery_view_with_cancellation(
        &self,
        context: &CapabilityGatewayRequestContext,
        cancellation: &CancellationToken,
    ) -> Result<Arc<[usize]>, rmcp::ErrorData> {
        run_until_cancelled(cancellation, self.discovery_view_uncancellable(context))
            .await
            .unwrap_or_else(|| Err(cancellation_error()))
    }

    async fn discovery_view_uncancellable(
        &self,
        context: &CapabilityGatewayRequestContext,
    ) -> Result<Arc<[usize]>, rmcp::ErrorData> {
        let cell = {
            let mut views = self.discovery_views.lock().await;
            if let Some(cell) = views.get(context) {
                Arc::clone(cell)
            } else {
                if views.len() >= MAX_DISCOVERY_CONTEXTS {
                    return Err(discovery_policy_error());
                }
                let cell = Arc::new(OnceCell::new());
                views.insert(context.clone(), Arc::clone(&cell));
                cell
            }
        };

        let policy = Arc::clone(&self.discovery_policy);
        let catalog = Arc::clone(&self.catalog);
        cell.get_or_try_init(|| async move {
            let mut visible = Vec::with_capacity(catalog.descriptors().len());
            for (index, descriptor) in catalog.descriptors().iter().enumerate() {
                if policy
                    .is_visible(descriptor, context)
                    .await
                    .map_err(|_| discovery_policy_error())?
                {
                    visible.push(index);
                }
            }
            Ok::<Arc<[usize]>, rmcp::ErrorData>(Arc::from(visible.into_boxed_slice()))
        })
        .await
        .map(Arc::clone)
    }

    fn visible_resource_uris(&self, view: &[usize]) -> std::collections::BTreeSet<String> {
        self.resources
            .iter()
            .filter(|(_, route)| descriptor_is_visible(view, route.descriptor_index))
            .map(|(uri, _)| uri.clone())
            .collect()
    }

    pub(crate) async fn dispatch(
        &self,
        name: &str,
        arguments: Option<JsonObject>,
        context: &CapabilityGatewayRequestContext,
        cancellation: &CancellationToken,
    ) -> Result<CallToolResult, rmcp::ErrorData> {
        if cancellation.is_cancelled() {
            return Ok(structured_error(
                MCP_CANCELLED_ERROR,
                "The Capability Gateway request was cancelled.",
            ));
        }
        let tool = self.tools.get(name).ok_or_else(|| {
            rmcp::ErrorData::invalid_params(
                "Capability Gateway Tool is not part of the immutable catalog.",
                None,
            )
        })?;
        let view = self
            .discovery_view_with_cancellation(context, cancellation)
            .await?;
        if !descriptor_is_visible(&view, tool.descriptor_index) {
            return Err(rmcp::ErrorData::invalid_params(
                "Capability Gateway Tool is not part of the immutable catalog.",
                None,
            ));
        }
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
        let result = match run_until_cancelled(
            cancellation,
            self.provider
                .authorize_and_invoke(descriptor, arguments, context),
        )
        .await
        {
            None => {
                return Ok(structured_error(
                    MCP_CANCELLED_ERROR,
                    "The Capability Gateway request was cancelled.",
                ));
            }
            Some(result) => match result {
                Ok(value) => Ok(value),
                Err(CapabilityGatewayInvocationFailure::Authorization(_)) => {
                    return Ok(structured_error(
                        MCP_AUTHORIZATION_ERROR,
                        "The Capability Gateway denied this invocation.",
                    ));
                }
                Err(CapabilityGatewayInvocationFailure::Invocation(error)) => Err(error),
            },
        };
        Ok(tool_result(result, &tool.output_schema))
    }
}
