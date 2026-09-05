//! Live routing for immutable Capability Gateway generations.
//!
//! A [`CapabilityGatewayMcpServer`] is intentionally frozen.  A host still
//! needs one stable service boundary while publishing a replacement catalog,
//! however.  This module keeps that boundary small: the factory swaps the
//! immutable server under a short synchronous lock, and the live adapter takes
//! one server snapshot at the start of each MCP operation.  An in-flight
//! operation therefore retains the old server (and its lease) until it
//! finishes, while the next discovery or invocation observes the replacement.

use std::sync::{Arc, RwLock};

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseError, UseResult};
use rmcp::model::{
    CallToolRequestParam, GetPromptRequestParam, GetPromptResult, ListPromptsResult,
    ListResourcesResult, ListToolsResult, PaginatedRequestParam, ReadResourceRequestParam,
    ReadResourceResult, ServerInfo,
};
use rmcp::{ServerHandler, ServiceExt};

use super::{
    CapabilityGatewayCatalogPublication, CapabilityGatewayCatalogStore, CapabilityGatewayMcpServer,
    CapabilityGatewayNotificationHub, CapabilityGatewayTransport,
};

const SESSION_STALE_ERROR: &str = "use.plugin.capability_gateway_session_stale";
const SESSION_INCOMPATIBLE_ERROR: &str = "use.plugin.capability_gateway_session_incompatible";
const SESSION_STATE_ERROR: &str = "use.plugin.capability_gateway_session_state";
const SESSION_PUBLICATION_ERROR: &str = "use.plugin.capability_gateway_session_publication";

/// The immutable catalog identity selected by a live session factory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewaySessionKey {
    pub installation: InstallationId,
    pub generation: u64,
    pub revision: String,
    pub digest: String,
}

/// Result of one atomic replacement of the immutable server source.
///
/// `catalog_changed` is false when the host replaces only the provider or
/// policy for the same catalog identity.  Such a replacement is useful after
/// reconnecting a provider, but it does not require MCP list-change
/// notifications.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewaySessionReplacement {
    pub previous: CapabilityGatewaySessionKey,
    pub current: CapabilityGatewaySessionKey,
    pub catalog_changed: bool,
    pub notification: Option<super::CapabilityGatewayNotificationReport>,
}

/// Host-owned source of immutable Gateway servers.
///
/// The factory is the safe cutover seam for a stateful MCP endpoint.  Hosts
/// must durably publish and verify the new catalog before calling
/// [`Self::replace`].  Replacement is serialized, swaps the source before the
/// standard MCP list-change fan-out, and runs that fan-out in a detached task
/// so caller cancellation cannot leave a new source installed without its
/// notification attempt.  Existing in-flight operations retain their cloned
/// old server and are not forcefully cancelled.
#[derive(Clone)]
pub struct CapabilityGatewaySessionFactory {
    current: Arc<RwLock<CapabilityGatewayMcpServer>>,
    cutover: Arc<tokio::sync::Mutex<()>>,
}

impl std::fmt::Debug for CapabilityGatewaySessionFactory {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewaySessionFactory")
            .field("current", &self.current().catalog())
            .finish_non_exhaustive()
    }
}

impl CapabilityGatewaySessionFactory {
    /// Start routing from one already validated immutable Gateway server.
    pub fn new(server: CapabilityGatewayMcpServer) -> Self {
        Self {
            current: Arc::new(RwLock::new(server)),
            cutover: Arc::new(tokio::sync::Mutex::new(())),
        }
    }

    /// Start routing from a server whose catalog has been verified against a
    /// durable payload-store publication.
    ///
    /// `CapabilityGatewayMcpServer::new` and [`Self::replace`] remain useful
    /// for hosts that own another persistence boundary. Hosts using the
    /// catalog payload store should prefer this constructor so a process
    /// cannot make an unpersisted in-memory projection visible by accident.
    pub async fn from_published(
        store: &CapabilityGatewayCatalogStore,
        publication: &CapabilityGatewayCatalogPublication,
        server: CapabilityGatewayMcpServer,
    ) -> UseResult<Self> {
        verify_published_server(store, publication, &server).await?;
        Ok(Self::new(server))
    }

    /// Snapshot the currently selected immutable server.
    ///
    /// The lock is never poisoned into a panic path: a poisoned read/write
    /// guard still contains the last fully assigned server, so its inner value
    /// is recovered explicitly.
    pub fn current(&self) -> CapabilityGatewayMcpServer {
        match self.current.read() {
            Ok(server) => server.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Return the shared list-change hub used by the current source.
    pub fn notification_hub(&self) -> Arc<CapabilityGatewayNotificationHub> {
        self.current().notification_hub()
    }

    /// Return the validated identity of the currently selected catalog.
    pub fn current_key(&self) -> UseResult<CapabilityGatewaySessionKey> {
        session_key(self.current().catalog())
    }

    /// Serve a live Gateway over stdin/stdout.
    ///
    /// A host that needs to publish a replacement while the service is
    /// running should retain a clone of this factory and call
    /// [`Self::replace`] from its lifecycle task. The stdio transport remains
    /// one process-owned MCP endpoint; individual operations still snapshot
    /// the selected immutable server.
    pub async fn serve_stdio(self) -> UseResult<()> {
        let service = self
            .live_server()
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|_| {
                UseError::new(
                    SESSION_STATE_ERROR,
                    "Failed to start the live Capability Gateway stdio service.",
                )
            })?;
        service.waiting().await.map_err(|_| {
            UseError::new(
                SESSION_STATE_ERROR,
                "The live Capability Gateway stdio service stopped with an error.",
            )
        })?;
        Ok(())
    }

    /// Atomically select a newer or same-generation immutable server.
    ///
    /// A lower publication generation is rejected.  Same-generation revisions
    /// are allowed because a projection can change without advancing the
    /// package lifecycle counter.  The consumer negotiation and lease mode
    /// are kept stable for one endpoint; changing either requires a new
    /// endpoint/factory so an existing client cannot silently change contract
    /// class or lose its generation fence.
    pub async fn replace(
        &self,
        next: CapabilityGatewayMcpServer,
    ) -> UseResult<CapabilityGatewaySessionReplacement> {
        let serial = Arc::clone(&self.cutover).lock_owned().await;
        let previous_server = self.current();
        let previous = session_key(previous_server.catalog())?;

        if previous_server.consumer_negotiation() != next.consumer_negotiation()
            || previous_server.snapshot_cursor().is_some() != next.snapshot_cursor().is_some()
        {
            return Err(UseError::new(
                SESSION_INCOMPATIBLE_ERROR,
                "The replacement Capability Gateway changes its consumer contract or lease mode.",
            ));
        }
        if next.catalog().installation() != &previous.installation {
            return Err(UseError::new(
                SESSION_INCOMPATIBLE_ERROR,
                "The replacement Capability Gateway belongs to another installation.",
            ));
        }
        if next.catalog().generation() < previous.generation {
            return Err(UseError::new(
                SESSION_STALE_ERROR,
                "The replacement Capability Gateway publication is older than the current source.",
            ));
        }

        // Every generation in one endpoint uses one hub.  This also makes a
        // caller that constructs a fresh server without remembering the hub
        // safe: the factory attaches its existing notification bus before the
        // source becomes visible.
        let next = next.with_notification_hub(previous_server.notification_hub())?;
        let current = session_key(next.catalog())?;
        let catalog_changed = previous != current;

        match self.current.write() {
            Ok(mut slot) => *slot = next,
            Err(poisoned) => *poisoned.into_inner() = next,
        }

        let notification = if catalog_changed {
            let hub = self.notification_hub();
            let catalog = self.current().catalog().clone();
            // Keep the serialization guard alive in an owned task.  If the
            // caller is cancelled after the swap, the task still advances the
            // hub and performs the bounded fan-out in publication order.
            let task = tokio::spawn(async move {
                let _serial = serial;
                hub.notify_catalog_changed(&catalog).await
            });
            match task.await {
                Ok(report) => Some(report?),
                Err(error) => {
                    return Err(UseError::new(
                        SESSION_STATE_ERROR,
                        format!("The Capability Gateway notification task failed: {error}"),
                    ));
                }
            }
        } else {
            drop(serial);
            None
        };

        Ok(CapabilityGatewaySessionReplacement {
            previous,
            current,
            catalog_changed,
            notification,
        })
    }

    /// Replace the live source only after proving that `next` is the exact
    /// consumer projection of bytes durably published in `store`.
    ///
    /// The store read is performed before the in-memory swap. A missing,
    /// tampered, cross-installation, or generation-mismatched payload is
    /// rejected without changing the current session source.
    pub async fn replace_published(
        &self,
        next: CapabilityGatewayMcpServer,
        store: &CapabilityGatewayCatalogStore,
        publication: &CapabilityGatewayCatalogPublication,
    ) -> UseResult<CapabilityGatewaySessionReplacement> {
        verify_published_server(store, publication, &next).await?;
        self.replace(next).await
    }

    /// Build a live adapter that resolves the current immutable server at the
    /// beginning of every MCP operation.  Existing operations retain the
    /// server snapshot they already acquired, so replacement is drain-safe.
    pub fn live_server(&self) -> CapabilityGatewayLiveMcpServer {
        CapabilityGatewayLiveMcpServer {
            factory: self.clone(),
            transport: CapabilityGatewayTransport::Stdio,
        }
    }
}

/// A standard MCP handler that delegates each operation to the factory's
/// current immutable server.
///
/// The type is public so hosts that own their own transport can use the same
/// cutover semantics as [`CapabilityGatewaySessionFactory::serve_streamable_http`].
#[derive(Clone)]
pub struct CapabilityGatewayLiveMcpServer {
    factory: CapabilityGatewaySessionFactory,
    transport: CapabilityGatewayTransport,
}

impl std::fmt::Debug for CapabilityGatewayLiveMcpServer {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayLiveMcpServer")
            .field("catalog", &self.factory.current().catalog())
            .field("transport", &self.transport)
            .finish()
    }
}

impl CapabilityGatewayLiveMcpServer {
    pub fn factory(&self) -> CapabilityGatewaySessionFactory {
        self.factory.clone()
    }

    pub(crate) fn with_transport(mut self, transport: CapabilityGatewayTransport) -> Self {
        self.transport = transport;
        self
    }

    fn snapshot(&self) -> CapabilityGatewayMcpServer {
        self.factory.current().with_transport(self.transport)
    }
}

impl ServerHandler for CapabilityGatewayLiveMcpServer {
    fn get_info(&self) -> ServerInfo {
        self.snapshot().get_info()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let server = self.snapshot();
        ServerHandler::call_tool(&server, request, request_context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let server = self.snapshot();
        ServerHandler::list_tools(&server, request, request_context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let server = self.snapshot();
        ServerHandler::list_resources(&server, request, request_context).await
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let server = self.snapshot();
        ServerHandler::list_prompts(&server, request, request_context).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let server = self.snapshot();
        ServerHandler::read_resource(&server, request, request_context).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let server = self.snapshot();
        ServerHandler::get_prompt(&server, request, request_context).await
    }

    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        let server = self.snapshot();
        ServerHandler::on_initialized(&server, context).await;
    }
}

fn session_key(catalog: &CapabilityGatewayCatalog) -> UseResult<CapabilityGatewaySessionKey> {
    catalog.validate()?;
    Ok(CapabilityGatewaySessionKey {
        installation: catalog.installation().clone(),
        generation: catalog.generation(),
        revision: catalog.revision().to_owned(),
        digest: catalog.descriptor_digest()?,
    })
}

async fn verify_published_server(
    store: &CapabilityGatewayCatalogStore,
    publication: &CapabilityGatewayCatalogPublication,
    server: &CapabilityGatewayMcpServer,
) -> UseResult<()> {
    publication.validate().map_err(|_| {
        UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The catalog publication identity is invalid.",
        )
    })?;
    if store.installation() != &publication.installation {
        return Err(UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The catalog publication belongs to another installation store.",
        ));
    }
    let Some(published) = store
        .get_exact(
            &publication.digest,
            publication.generation,
            &publication.revision,
        )
        .await
        .map_err(|_| {
            UseError::new(
                SESSION_PUBLICATION_ERROR,
                "The durable catalog publication could not be verified.",
            )
        })?
    else {
        return Err(UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The durable catalog publication is missing.",
        ));
    };
    let projected = published
        .for_consumer(server.consumer_negotiation())
        .map_err(|_| {
            UseError::new(
                SESSION_PUBLICATION_ERROR,
                "The durable catalog cannot be projected for this consumer.",
            )
        })?;
    if projected != *server.catalog()
        || server.catalog().installation() != &publication.installation
        || server.catalog().generation() != publication.generation
    {
        return Err(UseError::new(
            SESSION_PUBLICATION_ERROR,
            "The live Gateway catalog does not match the durable publication.",
        ));
    }
    Ok(())
}

const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<CapabilityGatewaySessionFactory>();
    assert_send_sync::<CapabilityGatewayLiveMcpServer>();
};
