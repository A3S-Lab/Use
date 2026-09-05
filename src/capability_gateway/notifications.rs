//! Standard MCP capability-list change notifications.
//!
//! The Gateway catalog is immutable for the lifetime of a server/session. A
//! host that publishes a newer catalog can nevertheless share this bounded
//! notification hub with the session factory and tell connected MCP clients
//! to re-list their capabilities. The hub carries no package data and does
//! not replace the host's session/catalog cutover: callers must install the
//! newer immutable server before (or together with) publishing the notice.

use std::sync::Arc;
use std::time::Duration;

use a3s_use_core::{CapabilityGatewayCatalog, InstallationId, UseError, UseResult};
use rmcp::service::{Peer, RoleServer};
use tokio::sync::Mutex;

const MAX_NOTIFICATION_PEERS: usize = 256;
const NOTIFICATION_SEND_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_NOTIFICATION_REVISION_BYTES: usize = 128;

/// A bounded result from one standard MCP list-change broadcast.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityGatewayNotificationReport {
    /// The publication generation supplied by the host.
    pub generation: u64,
    /// The canonical catalog revision supplied by the host.
    pub revision: String,
    /// Number of connected peers that received all three list-change
    /// notifications.
    pub notified_peers: usize,
    /// Number of peers removed because their transport was closed or did not
    /// accept the bounded notification send.
    pub removed_peers: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PublicationKey {
    generation: u64,
    revision: String,
}

#[derive(Clone)]
struct RegisteredPeer {
    id: u64,
    peer: Peer<RoleServer>,
}

/// Shared, bounded fan-out for standard MCP capability-list notifications.
///
/// A hub is tied to one installation and remembers its last publication key.
/// Replaying the same key, or a publication from an older generation, is a
/// safe no-op. Distinct revisions within one generation are allowed because a
/// capability projection can change without advancing the package lifecycle
/// counter. Peers are registered by the server's `initialized` callback;
/// closed and failed peers are retired on the next registration or broadcast.
#[derive(Clone)]
pub struct CapabilityGatewayNotificationHub {
    installation: InstallationId,
    peers: Arc<Mutex<Vec<RegisteredPeer>>>,
    state: Arc<Mutex<NotificationState>>,
}

#[derive(Debug)]
struct NotificationState {
    last: PublicationKey,
    next_peer_id: u64,
}

impl std::fmt::Debug for CapabilityGatewayNotificationHub {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CapabilityGatewayNotificationHub")
            .field("installation", &self.installation)
            .field(
                "peer_count",
                &self.peers.try_lock().map(|peers| peers.len()),
            )
            .field("state", &self.state.try_lock().ok())
            .finish()
    }
}

impl CapabilityGatewayNotificationHub {
    /// Create a hub whose deduplication state starts at `catalog`'s exact
    /// publication key.
    pub fn for_catalog(catalog: &CapabilityGatewayCatalog) -> UseResult<Self> {
        catalog.validate()?;
        let revision = catalog.revision().to_owned();
        validate_revision(&revision)?;
        Ok(Self {
            installation: catalog.installation().clone(),
            peers: Arc::new(Mutex::new(Vec::new())),
            state: Arc::new(Mutex::new(NotificationState {
                last: PublicationKey {
                    generation: catalog.generation(),
                    revision,
                },
                next_peer_id: 1,
            })),
        })
    }

    /// Return the installation identity guarded by this hub.
    pub fn installation(&self) -> &InstallationId {
        &self.installation
    }

    /// Register one initialized MCP server peer. Registration is bounded and
    /// prunes transports that have already closed. `false` means the peer was
    /// not retained because the bounded peer table is full.
    pub async fn register(&self, peer: Peer<RoleServer>) -> bool {
        let mut state = self.state.lock().await;
        let mut peers = self.peers.lock().await;
        peers.retain(|entry| !entry.peer.is_transport_closed());
        if peers.len() >= MAX_NOTIFICATION_PEERS {
            return false;
        }
        let id = state.next_peer_id;
        state.next_peer_id = state.next_peer_id.wrapping_add(1).max(1);
        peers.push(RegisteredPeer { id, peer });
        true
    }

    /// Broadcast standard MCP list-change notifications for an exact newer
    /// catalog. No private JSON-RPC message or package metadata is emitted.
    /// The host remains responsible for switching new sessions to `catalog`
    /// and retaining old generation leases until their sessions drain.
    pub async fn notify_catalog_changed(
        &self,
        catalog: &CapabilityGatewayCatalog,
    ) -> UseResult<CapabilityGatewayNotificationReport> {
        catalog.validate()?;
        if catalog.installation() != &self.installation {
            return Err(UseError::new(
                "use.plugin.capability_gateway_notification_scope_mismatch",
                "The Capability Gateway notification catalog belongs to another installation.",
            ));
        }
        let revision = catalog.revision().to_owned();
        validate_revision(&revision)?;
        let key = PublicationKey {
            generation: catalog.generation(),
            revision: revision.clone(),
        };

        // Serialize publication-key advancement so concurrent lifecycle
        // observers cannot announce the same key twice or regress to an older
        // generation. Distinct revisions at the same generation are valid and
        // must each be observable. The key is advanced before fan-out: a failed send is
        // recoverable by reconnecting clients, while repeated lifecycle
        // callbacks cannot create an unbounded notification storm.
        {
            let mut state = self.state.lock().await;
            if key.generation < state.last.generation
                || (key.generation == state.last.generation && key.revision == state.last.revision)
            {
                return Ok(CapabilityGatewayNotificationReport {
                    generation: key.generation,
                    revision,
                    notified_peers: 0,
                    removed_peers: 0,
                });
            }
            state.last = key;
        }

        let peers = {
            let peers = self.peers.lock().await;
            peers.clone()
        };
        // Fan out concurrently so one slow or back-pressured transport cannot
        // turn a bounded publication into `peer_count * timeout` latency.
        let mut tasks = tokio::task::JoinSet::new();
        for registered in peers {
            tasks.spawn(async move {
                let accepted =
                    !registered.peer.is_transport_closed() && notify_peer(&registered.peer).await;
                (registered.id, accepted)
            });
        }
        let mut notified_peers = 0_usize;
        let mut failed_ids = Vec::new();
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok((_id, true)) => notified_peers = notified_peers.saturating_add(1),
                Ok((id, false)) => failed_ids.push(id),
                Err(_) => {
                    // A task can only fail if the runtime aborts it or the
                    // peer future panics. There is no safe peer identity to
                    // retain in either case, so the corresponding entry is
                    // pruned by the next bounded registration sweep.
                }
            }
        }

        let removed_peers = if failed_ids.is_empty() {
            0
        } else {
            let mut peers = self.peers.lock().await;
            let before = peers.len();
            peers.retain(|entry| !failed_ids.contains(&entry.id));
            before.saturating_sub(peers.len())
        };

        Ok(CapabilityGatewayNotificationReport {
            generation: catalog.generation(),
            revision,
            notified_peers,
            removed_peers,
        })
    }

    /// Return the currently retained peer count after pruning closed
    /// transports. This is intended for bounded diagnostics and tests only.
    pub async fn peer_count(&self) -> usize {
        let mut peers = self.peers.lock().await;
        peers.retain(|entry| !entry.peer.is_transport_closed());
        peers.len()
    }
}

async fn notify_peer(peer: &Peer<RoleServer>) -> bool {
    let send = async {
        let (tools, resources, prompts) = tokio::join!(
            peer.notify_tool_list_changed(),
            peer.notify_resource_list_changed(),
            peer.notify_prompt_list_changed(),
        );
        tools.is_ok() && resources.is_ok() && prompts.is_ok()
    };
    tokio::time::timeout(NOTIFICATION_SEND_TIMEOUT, send)
        .await
        .is_ok_and(|result| result)
}

fn validate_revision(revision: &str) -> UseResult<()> {
    let digest = revision.strip_prefix("sha256:");
    if revision.len() > MAX_NOTIFICATION_REVISION_BYTES
        || digest.is_none_or(|digest| {
            digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        })
    {
        return Err(UseError::new(
            "use.plugin.capability_gateway_notification_revision_invalid",
            "The Capability Gateway notification revision is invalid.",
        ));
    }
    Ok(())
}
