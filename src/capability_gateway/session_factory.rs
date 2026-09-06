//! Live routing for immutable Capability Gateway generations.
//!
//! A [`CapabilityGatewayMcpServer`] is intentionally frozen.  A host still
//! needs one stable service boundary while publishing a replacement catalog,
//! however.  This module keeps that boundary small: the factory swaps the
//! immutable server under a short synchronous lock, and the live adapter takes
//! one server snapshot at the start of each MCP operation.  An in-flight
//! operation therefore retains the old server (and its lease) until it
//! finishes, while the next discovery or invocation observes the replacement.

use std::sync::{
    atomic::{AtomicU8, AtomicUsize, Ordering},
    Arc, OnceLock, RwLock,
};
use std::time::Duration;

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
const SESSION_DRAIN_TIMEOUT_ERROR: &str = "use.plugin.capability_gateway_session_drain_timeout";

const SESSION_RUNNING: u8 = 0;
const SESSION_DRAINING: u8 = 1;
const SESSION_DRAINED: u8 = 2;

/// Shared lifecycle gate for every live adapter clone of one endpoint.
///
/// The factory's immutable source owns the long-lived generation lease, while
/// each live MCP operation holds one short operation guard.  Moving to
/// `DRAINING` closes admission before waiting for those guards, so a caller
/// can release the source lease without racing a newly accepted request.
#[derive(Debug, Clone)]
struct SessionLifecycle {
    state: Arc<AtomicU8>,
    active: Arc<AtomicUsize>,
    changed: Arc<tokio::sync::Notify>,
}

#[derive(Debug)]
struct SessionOperationGuard {
    lifecycle: SessionLifecycle,
}

impl SessionLifecycle {
    fn new() -> Self {
        Self {
            state: Arc::new(AtomicU8::new(SESSION_RUNNING)),
            active: Arc::new(AtomicUsize::new(0)),
            changed: Arc::new(tokio::sync::Notify::new()),
        }
    }

    fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    fn enter(&self) -> UseResult<SessionOperationGuard> {
        loop {
            if self.state() != SESSION_RUNNING {
                return Err(session_state_error(
                    "The Capability Gateway session is draining or already drained.",
                ));
            }
            let active = self.active.load(Ordering::Acquire);
            if active == usize::MAX {
                return Err(session_state_error(
                    "The Capability Gateway session has reached its active-operation bound.",
                ));
            }
            if self
                .active
                .compare_exchange(active, active + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_err()
            {
                continue;
            }
            // Close the small race in which drain changes state after the
            // first check but before the active count is incremented.
            if self.state() == SESSION_RUNNING {
                return Ok(SessionOperationGuard {
                    lifecycle: self.clone(),
                });
            }
            self.leave();
            return Err(session_state_error(
                "The Capability Gateway session started draining before the request was admitted.",
            ));
        }
    }

    fn leave(&self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
        self.changed.notify_waiters();
    }

    async fn wait_until_idle(&self) {
        loop {
            // Create the waiter before checking the counter.  The final
            // operation may leave between the counter read and the await;
            // `Notify::notified` created first is guaranteed to observe the
            // ensuing `notify_waiters` call instead of losing that wake-up.
            let notified = self.changed.notified();
            if self.active.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}

impl Drop for SessionOperationGuard {
    fn drop(&mut self) {
        self.lifecycle.leave();
    }
}

fn session_state_error(message: impl Into<String>) -> UseError {
    UseError::new(SESSION_STATE_ERROR, message)
}

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
    lifecycle: SessionLifecycle,
    /// A one-shot proof that this factory was drained while it still held a
    /// host-owned external lease for the recorded endpoint identity.  The
    /// live server intentionally drops that lease after draining, but a
    /// lifecycle retry still needs to prove that it is retrying the same
    /// already-retired Control endpoint rather than an unbound copy.
    drained_external_key: Arc<OnceLock<CapabilityGatewaySessionKey>>,
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
            lifecycle: SessionLifecycle::new(),
            drained_external_key: Arc::new(OnceLock::new()),
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

    /// Stop admitting new operations, wait for already admitted operations to
    /// finish, and release the source server's generation lease.
    ///
    /// The transition is serialized with [`Self::replace`].  A timeout leaves
    /// the factory in the draining state, so a later call can continue waiting
    /// without reopening admission.  Once drained, the current server remains
    /// available for diagnostics but is deliberately unleased; its live
    /// adapter rejects all new requests.  Callers that retained independent
    /// clones of the immutable server still own those clones and their leases
    /// until they drop them.
    ///
    /// A zero timeout is useful for a non-blocking shutdown probe: it succeeds
    /// when no operation is currently admitted and returns the normal timeout
    /// error when work is still in flight.
    pub async fn drain(&self, timeout: Duration) -> UseResult<()> {
        let _serial = self.cutover.clone().lock_owned().await;
        self.drain_locked(timeout).await
    }

    /// Drain only when the currently selected source is the exact endpoint
    /// bound to `expected`.  The identity and lease checks run while holding
    /// the same serialization guard as replacement and the draining state
    /// transition, so a concurrent replacement cannot slip between validation
    /// and admission closure.  `false` means the source was not the expected
    /// Control-bound endpoint and was left untouched.
    pub(crate) async fn drain_if_bound(
        &self,
        expected: &CapabilityGatewaySessionKey,
        timeout: Duration,
    ) -> UseResult<bool> {
        let _serial = self.cutover.clone().lock_owned().await;
        let bound = if self.lifecycle.state() == SESSION_DRAINED {
            // `drain_locked` has already detached the external lease.  A
            // successful first attempt leaves this typed proof behind so an
            // exact retry can remain idempotent without accepting a copied
            // unleased catalog.
            self.drained_external_key
                .get()
                .is_some_and(|key| key == expected)
        } else {
            let current = self.current();
            session_key(current.catalog())? == *expected
                && current.generation_lease_mode()
                    == super::CapabilityGatewayGenerationLeaseMode::External
                && current.external_lease_matches(expected)
        };
        if !bound {
            return Ok(false);
        }
        self.drain_locked(timeout).await?;
        Ok(true)
    }

    async fn drain_locked(&self, timeout: Duration) -> UseResult<()> {
        match self.lifecycle.state() {
            SESSION_DRAINED => return Ok(()),
            SESSION_RUNNING => {
                self.lifecycle
                    .state
                    .compare_exchange(
                        SESSION_RUNNING,
                        SESSION_DRAINING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .map_err(|_| {
                        session_state_error(
                            "The Capability Gateway session changed state while draining.",
                        )
                    })?;
            }
            SESSION_DRAINING => {}
            _ => {
                return Err(session_state_error(
                    "The Capability Gateway session has an unknown lifecycle state.",
                ));
            }
        }

        // Do not hand an already-idle session to `timeout(Duration::ZERO, ..)`.
        // Tokio is allowed to return the timeout before polling the future,
        // which would turn an immediately drainable session into a spurious
        // failure.  Admission is closed above, so an observed zero count
        // cannot be invalidated by a newly entered operation.
        let active = self.lifecycle.active.load(Ordering::Acquire);
        if active != 0
            && tokio::time::timeout(timeout, self.lifecycle.wait_until_idle())
                .await
                .is_err()
        {
            return Err(UseError::new(
                SESSION_DRAIN_TIMEOUT_ERROR,
                "The Capability Gateway session did not drain before the supplied deadline.",
            ));
        }

        // Capture the identity and whether the lease was externally proven in
        // a short scope.  Keeping a cloned server alive while replacing the
        // factory slot would retain the same generation lease and make the
        // subsequent retention fence wait on itself.
        let (key, externally_bound, detached) = {
            let current = self.current();
            let key = session_key(current.catalog())?;
            let externally_bound = current.generation_lease_mode()
                == super::CapabilityGatewayGenerationLeaseMode::External
                && current.external_lease_matches(&key);
            (key, externally_bound, current.without_generation_lease())
        };
        if externally_bound {
            // The lifecycle state is monotonic, so a failed set would only
            // indicate corruption or an impossible second transition. Keep
            // the first proof; it is safer than replacing it during replay.
            let _ = self.drained_external_key.set(key);
        }
        self.lifecycle
            .state
            .store(SESSION_DRAINED, Ordering::Release);
        match self.current.write() {
            Ok(mut slot) => *slot = detached,
            Err(poisoned) => *poisoned.into_inner() = detached,
        }
        self.lifecycle.changed.notify_waiters();
        Ok(())
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
        self.replace_locked(next, serial).await
    }

    /// Replace the source only if it is still the exact server identified by
    /// `expected`.  `None` means another local cutover won the race and the
    /// caller must refresh its durable publication before retrying.  The
    /// compare-and-swap is deliberately scoped to the factory's serialization
    /// guard; it prevents a stale same-generation build from overwriting a
    /// newer local replacement.
    pub(crate) async fn replace_if_current(
        &self,
        expected: &CapabilityGatewaySessionKey,
        next: CapabilityGatewayMcpServer,
    ) -> UseResult<Option<CapabilityGatewaySessionReplacement>> {
        let serial = Arc::clone(&self.cutover).lock_owned().await;
        if self.lifecycle.state() != SESSION_RUNNING {
            return Err(session_state_error(
                "The Capability Gateway session is draining or already drained.",
            ));
        }
        let current = session_key(self.current().catalog())?;
        if current != *expected {
            return Ok(None);
        }
        Ok(Some(self.replace_locked(next, serial).await?))
    }

    async fn replace_locked(
        &self,
        next: CapabilityGatewayMcpServer,
        serial: tokio::sync::OwnedMutexGuard<()>,
    ) -> UseResult<CapabilityGatewaySessionReplacement> {
        if self.lifecycle.state() != SESSION_RUNNING {
            return Err(session_state_error(
                "The Capability Gateway session is draining or already drained.",
            ));
        }
        let previous_server = self.current();
        let previous = session_key(previous_server.catalog())?;

        if previous_server.consumer_negotiation() != next.consumer_negotiation()
            || previous_server.generation_lease_mode() != next.generation_lease_mode()
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

    fn enter_operation(&self) -> UseResult<SessionOperationGuard> {
        self.lifecycle.enter()
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

    fn snapshot(&self) -> UseResult<(CapabilityGatewayMcpServer, SessionOperationGuard)> {
        let operation = self.factory.enter_operation()?;
        Ok((
            self.factory.current().with_transport(self.transport),
            operation,
        ))
    }
}

impl ServerHandler for CapabilityGatewayLiveMcpServer {
    fn get_info(&self) -> ServerInfo {
        // `get_info` is a local protocol description and has no provider or
        // payload side effect.  Keep it available while draining so clients
        // can observe the endpoint's final server metadata.
        self.factory
            .current()
            .with_transport(self.transport)
            .get_info()
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::call_tool(&server, request, request_context).await
    }

    async fn list_tools(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListToolsResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::list_tools(&server, request, request_context).await
    }

    async fn list_resources(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListResourcesResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::list_resources(&server, request, request_context).await
    }

    async fn list_prompts(
        &self,
        request: Option<PaginatedRequestParam>,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ListPromptsResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::list_prompts(&server, request, request_context).await
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<ReadResourceResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::read_resource(&server, request, request_context).await
    }

    async fn get_prompt(
        &self,
        request: GetPromptRequestParam,
        request_context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<GetPromptResult, rmcp::ErrorData> {
        let (server, _operation) = self.snapshot().map_err(session_state_error_data)?;
        ServerHandler::get_prompt(&server, request, request_context).await
    }

    async fn on_initialized(&self, context: rmcp::service::NotificationContext<rmcp::RoleServer>) {
        let Ok((server, _operation)) = self.snapshot() else {
            return;
        };
        ServerHandler::on_initialized(&server, context).await;
    }
}

fn session_state_error_data(_error: UseError) -> rmcp::ErrorData {
    rmcp::ErrorData::invalid_request(
        "The Capability Gateway session is draining or already drained.",
        Some(serde_json::json!({ "code": SESSION_STATE_ERROR })),
    )
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

#[cfg(test)]
mod tests {
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use std::time::Duration;

    use a3s_use_core::{
        CapabilityDescriptor, CapabilityGatewayCatalog, InstallationId, InstallationKind, UseResult,
    };
    use async_trait::async_trait;
    use serde_json::Value;

    use super::super::{
        CapabilityGatewayExternalLease, CapabilityGatewayInvocationProvider,
        CapabilityGatewayRequestContext, CapabilityGatewaySessionFactory,
    };
    use super::{SESSION_DRAINING, SESSION_DRAIN_TIMEOUT_ERROR, SESSION_STATE_ERROR};

    struct NoopProvider;

    #[async_trait]
    impl CapabilityGatewayInvocationProvider for NoopProvider {
        async fn authorize(
            &self,
            _descriptor: &CapabilityDescriptor,
            _arguments: &Value,
            _context: &CapabilityGatewayRequestContext,
        ) -> UseResult<()> {
            Ok(())
        }

        async fn invoke(
            &self,
            _descriptor: &CapabilityDescriptor,
            _arguments: Value,
            _context: &CapabilityGatewayRequestContext,
        ) -> UseResult<Value> {
            Ok(Value::Null)
        }
    }

    struct LeaseMarker(Arc<AtomicBool>);

    impl CapabilityGatewayExternalLease for LeaseMarker {
        fn matches_gateway_session(
            &self,
            _key: &super::super::CapabilityGatewaySessionKey,
        ) -> bool {
            false
        }
    }

    impl Drop for LeaseMarker {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    struct BoundLeaseMarker {
        key: super::super::CapabilityGatewaySessionKey,
        dropped: Arc<AtomicBool>,
    }

    impl CapabilityGatewayExternalLease for BoundLeaseMarker {
        fn matches_gateway_session(&self, key: &super::super::CapabilityGatewaySessionKey) -> bool {
            self.key == *key
        }
    }

    impl Drop for BoundLeaseMarker {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn factory(dropped: Arc<AtomicBool>) -> CapabilityGatewaySessionFactory {
        let installation = InstallationId::new(InstallationKind::User, "session-drain").unwrap();
        let catalog = CapabilityGatewayCatalog::new(installation, 1, Vec::new()).unwrap();
        let server = super::super::CapabilityGatewayMcpServer::new(catalog, Arc::new(NoopProvider))
            .unwrap()
            .with_external_lease(Arc::new(LeaseMarker(dropped)))
            .unwrap();
        CapabilityGatewaySessionFactory::new(server)
    }

    #[tokio::test]
    async fn drain_closes_admission_waits_and_detaches_generation_lease() {
        let dropped = Arc::new(AtomicBool::new(false));
        let factory = factory(Arc::clone(&dropped));
        let active = factory.enter_operation().unwrap();

        let error = factory.drain(Duration::from_millis(20)).await.unwrap_err();
        assert_eq!(error.code, SESSION_DRAIN_TIMEOUT_ERROR);
        assert_eq!(factory.lifecycle.state(), SESSION_DRAINING);
        assert!(factory.enter_operation().is_err());

        drop(active);
        factory.drain(Duration::from_secs(1)).await.unwrap();
        assert_eq!(
            factory.current().generation_lease_mode(),
            super::super::CapabilityGatewayGenerationLeaseMode::None
        );
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(
            factory.enter_operation().unwrap_err().code,
            SESSION_STATE_ERROR
        );
    }

    #[tokio::test]
    async fn replacement_is_rejected_after_drain_begins() {
        let dropped = Arc::new(AtomicBool::new(false));
        let factory = factory(dropped);
        factory.drain(Duration::from_secs(1)).await.unwrap();
        let error = factory.replace(factory.current()).await.unwrap_err();
        assert_eq!(error.code, SESSION_STATE_ERROR);
    }

    #[tokio::test]
    async fn zero_timeout_drains_an_already_idle_session() {
        let dropped = Arc::new(AtomicBool::new(false));
        let factory = factory(Arc::clone(&dropped));

        factory.drain(Duration::ZERO).await.unwrap();

        assert_eq!(
            factory.current().generation_lease_mode(),
            super::super::CapabilityGatewayGenerationLeaseMode::None
        );
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn bound_drain_replay_uses_only_the_proven_external_identity() {
        let installation =
            InstallationId::new(InstallationKind::User, "session-bound-drain").unwrap();
        let catalog = CapabilityGatewayCatalog::new(installation, 1, Vec::new()).unwrap();
        let key = super::session_key(&catalog).unwrap();
        let dropped = Arc::new(AtomicBool::new(false));
        let server = super::super::CapabilityGatewayMcpServer::new(catalog, Arc::new(NoopProvider))
            .unwrap()
            .with_external_lease(Arc::new(BoundLeaseMarker {
                key: key.clone(),
                dropped: Arc::clone(&dropped),
            }))
            .unwrap();
        let factory = CapabilityGatewaySessionFactory::new(server);

        assert!(factory.drain_if_bound(&key, Duration::ZERO).await.unwrap());
        assert!(dropped.load(Ordering::SeqCst));
        assert!(factory.drain_if_bound(&key, Duration::ZERO).await.unwrap());

        let mut unrelated = key;
        unrelated.generation += 1;
        assert!(!factory
            .drain_if_bound(&unrelated, Duration::ZERO)
            .await
            .unwrap());
    }

    #[tokio::test]
    async fn direct_unbound_drain_does_not_forge_a_replay_binding() {
        let dropped = Arc::new(AtomicBool::new(false));
        let factory = factory(Arc::clone(&dropped));
        let key = factory.current_key().unwrap();

        factory.drain(Duration::ZERO).await.unwrap();

        assert!(dropped.load(Ordering::SeqCst));
        assert!(!factory.drain_if_bound(&key, Duration::ZERO).await.unwrap());
    }

    #[tokio::test]
    async fn conditional_replacement_does_not_overwrite_a_concurrent_cutover() {
        let installation =
            InstallationId::new(InstallationKind::User, "session-conditional-replace").unwrap();
        let initial = CapabilityGatewayCatalog::new(installation.clone(), 1, Vec::new()).unwrap();
        let winner = CapabilityGatewayCatalog::new(installation.clone(), 2, Vec::new()).unwrap();
        let stale_build = CapabilityGatewayCatalog::new(installation, 3, Vec::new()).unwrap();
        let factory = CapabilityGatewaySessionFactory::new(
            super::super::CapabilityGatewayMcpServer::new(initial, Arc::new(NoopProvider)).unwrap(),
        );
        let expected = factory.current_key().unwrap();

        factory
            .replace(
                super::super::CapabilityGatewayMcpServer::new(
                    winner.clone(),
                    Arc::new(NoopProvider),
                )
                .unwrap(),
            )
            .await
            .unwrap();

        let replacement = factory
            .replace_if_current(
                &expected,
                super::super::CapabilityGatewayMcpServer::new(stale_build, Arc::new(NoopProvider))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert!(replacement.is_none());
        assert_eq!(factory.current().catalog(), &winner);
    }
}
