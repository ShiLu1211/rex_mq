//! Forwarder port — owns cross-node message delivery.
//!
//! Extracted from `ServerClusterManager` per ADR-0002 (C4 split). Three
//! responsibilities, all async because they touch the cluster wire:
//!
//! - [`Forwarder::forward`] — outbound cluster send; takes the targeted
//!   peer (resolved upstream by the `Router`) and a `ForwardRequest`.
//!   Returns a [`FwdResult`] so callers can branch on actual failure
//!   modes rather than guessing from a bool. The fallback walk over
//!   other known peers lives inside this method.
//! - [`Forwarder::deliver`] — inbound peer-message relay to local
//!   subscribers; returns a [`DeliveryOutcome`] with delivery counts
//!   and the ack-required flag. **ACKs are not broadcast from inside
//!   `deliver`;** the dispatch loop in `ServerClusterManager::handle_messages`
//!   reads the outcome and decides whether to broadcast back. Keeping
//!   the ACK responsibility outside `NetworkForwarder` avoids the need
//!   for a self-reference for the broadcast path.
//! - [`Forwarder::broadcast`] — cluster-internal fan-out (e.g.
//!   `TitleRegister` announcements). Counts successful sends.
//!
//! ## Lifecycle
//!
//! `NetworkForwarder` holds a slot for the cluster's `NodeManager`
//! (`Arc<ArcSwap<Option<Arc<NodeManager>>>>`). The slot is empty at
//! construction and populated when the cluster starts. Calls to
//! `forward` before population return
//! `FwdResult::PeerUnreachable("cluster-not-started")` so callers can
//! detect this state without inspecting internals.

use std::net::SocketAddr;
use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use bytes::BytesMut;
use rex_cluster::node::NodeManager;
use rex_cluster::route_table::GlobalRouteTable;
use rex_cluster::types::{ClusterMessage, ForwardMessage};
use rex_core::RexData;
use tracing::{debug, info, warn};

use crate::ForwardRequest;
use crate::system::client_registry::ClientRegistry;

/// Outcome of an outbound [`Forwarder::forward`] call. Replaces the
/// original `bool` that hid why a send failed (introduced in
/// `handler/title.rs::fallback-loop`).
///
/// The variants expose the failure modes the title handler always
/// wanted but couldn't see. `PeerUnreachable(String)` carries the
/// failing node's name so the fallback loop can skip it without
/// re-asking the route table.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "FwdResult carries the failure mode; ignoring it loses information"]
pub enum FwdResult {
    /// Message queued on the target node's incoming channel.
    Delivered,
    /// Cluster route table has no node for the targeted title.
    NoPeerForTitle,
    /// The chosen peer is connected but failed to accept the message
    /// (the retry path was exhausted). The inner string is the
    /// failing node's id, so callers can skip it during fallback.
    PeerUnreachable(String),
    /// Peer acknowledged it but rejected the request body (e.g.
    /// payload too large, schema mismatch). Inner string is the
    /// failing peer's id.
    PeerRejected(String),
}

/// Counts and ack state for an inbound [`Forwarder::deliver`] call.
/// Replaces a void return whose outcome could only be read from logs.
///
/// `acked_back` mirrors `ForwardMessage::require_ack` — set `true`
/// when the dispatch loop should broadcast a `ForwardAck`. The actual
/// broadcast lives outside `deliver` (see module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryOutcome {
    /// Number of local subscribers the message was sent to.
    pub delivered_to: usize,
    /// Number of local subscribers where `send_buf` failed.
    pub failed: usize,
    /// Whether the dispatch loop should broadcast a `ForwardAck`
    /// back. Mirrors `ForwardMessage::require_ack`.
    pub acked_back: bool,
}

/// Cross-node message delivery. Split out from `ClusterPort` per
/// ADR-0002 when `cluster/forward_relay.rs` appeared as the second
/// consumer that bypassed the port.
#[async_trait]
pub trait Forwarder: Send + Sync {
    /// Outbound: send `req` to `target_node`. If that peer fails
    /// (transport error), walk all known connected peers — excluding
    /// `target_node` and self — and try each until one accepts.
    async fn forward(&self, target_node: &str, req: &ForwardRequest) -> FwdResult;

    /// Inbound: relay a peer-sent `ForwardMessage` to local
    /// subscribers. ACKs are **not** broadcast from here; the
    /// dispatch loop in `ServerClusterManager::handle_messages`
    /// observes the returned `DeliveryOutcome` and decides.
    async fn deliver(&self, msg: &ForwardMessage) -> DeliveryOutcome;

    /// Cluster-internal fan-out (e.g. `TitleRegister`).
    /// Returns the number of sends the transport accepted (best-effort,
    /// may be `0` if the cluster is not started).
    async fn broadcast(&self, msg: &ClusterMessage) -> usize;
}

/// Production [`Forwarder`] implementation.
///
/// Holds an `ArcSwap<Option<Arc<NodeManager>>>` slot — empty at
/// construction, populated by `ServerClusterManager::start()`. Reads
/// use `ArcSwap::load` for an async-lock-free hot path.
pub struct NetworkForwarder {
    node_manager: Arc<ArcSwap<Option<Arc<NodeManager>>>>,
    route_table: Arc<GlobalRouteTable>,
    local_node_id: rex_cluster::types::NodeId,
    client_registry: Arc<dyn ClientRegistry>,
}

impl NetworkForwarder {
    /// Construct a new `NetworkForwarder`. The `node_manager` slot is
    /// empty — populate it via [`NetworkForwarder::set_node_manager`]
    /// once the cluster has started.
    pub fn new(
        node_manager: Arc<ArcSwap<Option<Arc<NodeManager>>>>,
        route_table: Arc<GlobalRouteTable>,
        local_node_id: rex_cluster::types::NodeId,
        client_registry: Arc<dyn ClientRegistry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            node_manager,
            route_table,
            local_node_id,
            client_registry,
        })
    }

    /// Populate the cluster's `NodeManager` slot. Called by
    /// `ServerClusterManager::start` when the cluster boots, and
    /// cleared on shutdown.
    pub fn set_node_manager(&self, nm: Option<Arc<NodeManager>>) {
        // `ArcSwap` stores `Arc<T>` internally; wrap the Option so
        // the slot type matches `Arc<Option<Arc<NodeManager>>>`.
        self.node_manager.store(Arc::new(nm));
    }

    /// Load the current node manager, if any. The hot path on
    /// `forward` uses this directly.
    fn node_manager(&self) -> Option<Arc<NodeManager>> {
        // `load` returns the inner `Arc<Option<Arc<NodeManager>>>`;
        // `as_ref().clone()` peels one Arc and clones the Option.
        self.node_manager.load().as_ref().clone()
    }

    /// Walk all known connected peers (excluding `exclude` and self),
    /// trying each until one accepts. Returns `Delivered` on the first
    /// success; `NoPeerForTitle` if no other known peer has an
    /// address; `PeerUnreachable(excluded)` if every other peer also
    /// failed.
    async fn fallback_excluding(&self, req: &ForwardRequest, exclude: Option<&str>) -> FwdResult {
        let nm = match self.node_manager() {
            Some(nm) => nm,
            None => return FwdResult::NoPeerForTitle,
        };
        let transport = nm.get_transport();
        let local_id = self.local_node_id.to_string();

        for other in transport.connected_nodes() {
            if other == local_id || Some(other.as_str()) == exclude {
                continue;
            }
            let Some(addr_str) = self.route_table.get_node_addr(&other) else {
                continue;
            };
            let Ok(addr) = addr_str.parse::<SocketAddr>() else {
                continue;
            };
            if try_send(&nm, &other, addr, req).await {
                return FwdResult::Delivered;
            }
        }
        match exclude {
            Some(node) => FwdResult::PeerUnreachable(node.to_string()),
            None => FwdResult::NoPeerForTitle,
        }
    }
}

#[async_trait]
impl Forwarder for NetworkForwarder {
    async fn forward(&self, target_node: &str, req: &ForwardRequest) -> FwdResult {
        // 1. Cluster must be started.
        let nm = match self.node_manager() {
            Some(nm) => nm,
            None => return FwdResult::PeerUnreachable("cluster-not-started".into()),
        };

        // 2. Look up the targeted peer's address. Unknown peer →
        //    try the fallback walk.
        let Some(addr_str) = self.route_table.get_node_addr(target_node) else {
            return self.fallback_excluding(req, None).await;
        };
        let addr = match addr_str.parse::<SocketAddr>() {
            Ok(a) => a,
            Err(e) => {
                warn!(
                    "Route table has unparseable addr for {}: {}",
                    target_node, e
                );
                return self.fallback_excluding(req, Some(target_node)).await;
            }
        };

        // 3. Try the targeted peer.
        if try_send(&nm, target_node, addr, req).await {
            return FwdResult::Delivered;
        }

        // 4. Targeted peer refused — walk other known peers.
        self.fallback_excluding(req, Some(target_node)).await
    }

    async fn deliver(&self, msg: &ForwardMessage) -> DeliveryOutcome {
        // Save fields before moving into the dispatch branches.
        let title = msg.title.clone();
        let is_group = msg.is_group;
        let is_broadcast = msg.is_broadcast;
        let target_client_id = msg.target_client_id;
        let require_ack = msg.require_ack;

        // Unpack the payload once and reuse for every subscriber.
        let rex_data = RexData::unpack(BytesMut::from(msg.payload.as_slice()));
        let buf = rex_data.pack_ref();

        let mut outcome = DeliveryOutcome {
            acked_back: require_ack,
            ..DeliveryOutcome::default()
        };

        // Broadcast / Group: deliver to every local subscriber.
        if is_broadcast || is_group {
            let clients = self.client_registry.find_all_by_title(&title, None);
            if clients.is_empty() {
                warn!(
                    "No local subscribers found for broadcast/group title: {}",
                    title
                );
                return outcome;
            }
            info!(
                "Delivering {} message to {} local subscribers",
                if is_broadcast { "broadcast" } else { "group" },
                clients.len()
            );
            for client in clients {
                let client_id = client.id();
                match client.send_buf(buf).await {
                    Ok(()) => outcome.delivered_to += 1,
                    Err(e) => {
                        warn!("Failed to send to client {:032x}: {}", client_id, e);
                        outcome.failed += 1;
                    }
                }
            }
            return outcome;
        }

        // Unicast: target by id if known, else by title.
        let target_client = if target_client_id != 0 {
            self.client_registry.find_some_by_id(target_client_id)
        } else {
            self.client_registry.find_one_by_title(&title, None)
        };

        match target_client {
            Some(client) => {
                let client_id = client.id();
                info!(
                    "Delivering forwarded message to local client {:032x}",
                    client_id
                );
                match client.send_buf(buf).await {
                    Ok(()) => outcome.delivered_to += 1,
                    Err(e) => {
                        warn!("Failed to send to client {:032x}: {}", client_id, e);
                        outcome.failed += 1;
                    }
                }
            }
            None => {
                warn!("No local subscriber found for title: {}", title);
            }
        }

        outcome
    }

    async fn broadcast(&self, msg: &ClusterMessage) -> usize {
        let nm = match self.node_manager() {
            Some(nm) => nm,
            None => return 0,
        };
        let transport = nm.get_transport();
        let connected = transport.connected_nodes();
        let local_id = self.local_node_id.to_string();
        let mut count = 0;
        for node_id in connected {
            if node_id == local_id {
                continue;
            }
            match transport.send_to(&node_id, msg).await {
                Ok(()) => count += 1,
                Err(e) => warn!("Failed to broadcast to node {}: {}", node_id, e),
            }
        }
        count
    }
}

/// Send `req` over the cluster transport to `target_node` at `addr`.
/// Returns `true` if the transport accepted the message. On failure,
/// attempts a single reconnect-and-retry before giving up. Mirrors
/// the original `ServerClusterManager::forward_message` wire-level
/// logic.
async fn try_send(
    nm: &Arc<NodeManager>,
    target_node: &str,
    addr: SocketAddr,
    req: &ForwardRequest,
) -> bool {
    let transport = nm.get_transport();
    if !transport.is_connected(target_node) {
        warn!(
            "Not connected to target node {} (cluster transport)",
            target_node
        );
        return false;
    }

    let forward_msg = rex_cluster::types::ForwardMessage {
        forward_id: fastrand::u64(..),
        original_source: req.source_client_id,
        target_client_id: req.target_client_id,
        title: req.title.clone(),
        payload: req.payload.clone(),
        is_group: matches!(req.msg_type, crate::ForwardType::Group),
        is_broadcast: matches!(req.msg_type, crate::ForwardType::Broadcast),
        require_ack: false,
    };
    let cluster_msg = ClusterMessage::Forward(forward_msg);

    match transport.send_to(target_node, &cluster_msg).await {
        Ok(()) => {
            debug!("Forwarded message to node {} at {}", target_node, addr);
            true
        }
        Err(e) => {
            warn!("Failed to forward message to {}: {}", target_node, e);
            // Try to reconnect and retry once.
            if try_reconnect_and_send(&transport, target_node, addr, &cluster_msg).await {
                info!(
                    "Successfully reconnected and sent message to {}",
                    target_node
                );
                true
            } else {
                false
            }
        }
    }
}

async fn try_reconnect_and_send(
    transport: &Arc<rex_cluster::transport::ClusterTransport>,
    node_id: &str,
    addr: SocketAddr,
    msg: &ClusterMessage,
) -> bool {
    transport.remove_connection(node_id);
    match transport.connect(node_id.to_string().into(), addr).await {
        Ok(()) => transport.send_to(node_id, msg).await.is_ok(),
        Err(e) => {
            warn!("Failed to reconnect to {}: {}", node_id, e);
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cluster::forward::{ForwardRequest, ForwardType};
    use crate::handler::test_util::dummy_client_with_id;
    use crate::system::client_registry::ClientRegistryImpl;

    fn local_routing_table() -> Arc<GlobalRouteTable> {
        let local_id = rex_cluster::types::NodeId::new("local-node");
        Arc::new(GlobalRouteTable::with_local_node(local_id))
    }

    fn empty_forwarder() -> Arc<NetworkForwarder> {
        let slot = Arc::new(ArcSwap::from_pointee(None));
        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        NetworkForwarder::new(
            slot,
            local_routing_table(),
            rex_cluster::types::NodeId::new("local-node"),
            registry,
        )
    }

    fn sample_forward_request(title: &str) -> ForwardRequest {
        ForwardRequest {
            source_client_id: 0xAAu128,
            target_client_id: 0,
            title: title.to_string(),
            payload: vec![1, 2, 3, 4],
            msg_type: ForwardType::Unicast,
        }
    }

    #[tokio::test]
    async fn forward_with_unstarted_cluster_returns_cluster_not_started() {
        let forwarder = empty_forwarder();
        let req = sample_forward_request("any_title");
        match forwarder.forward("peer-1", &req).await {
            FwdResult::PeerUnreachable(reason) => {
                assert_eq!(reason, "cluster-not-started");
            }
            other => panic!(
                "expected PeerUnreachable(cluster-not-started), got {:?}",
                other
            ),
        }
    }

    #[tokio::test]
    async fn forward_to_unknown_target_when_unstarted_returns_cluster_not_started() {
        let forwarder = empty_forwarder();
        let req = sample_forward_request("any_title");
        // Empty slot dominates the early-return; even an unknown
        // target returns cluster-not-started.
        let result = forwarder.forward("unknown-target", &req).await;
        assert!(matches!(result, FwdResult::PeerUnreachable(_)));
    }

    #[tokio::test]
    async fn broadcast_with_unstarted_cluster_returns_zero() {
        let forwarder = empty_forwarder();
        let count = forwarder
            .broadcast(&ClusterMessage::Ping(rex_cluster::types::PingMessage {
                node_id: "local-node".into(),
                timestamp: 0,
            }))
            .await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn deliver_with_no_subscribers_returns_zero_counts_and_no_ack() {
        let forwarder = empty_forwarder();
        let rex_data = rex_core::RexData::new(rex_core::RexCommand::Title, "absent", b"hello");
        let fwd = ForwardMessage {
            forward_id: 1,
            original_source: 0xAAu128,
            target_client_id: 0,
            title: "absent".into(),
            payload: rex_data.pack_ref().to_vec(),
            is_group: false,
            is_broadcast: false,
            require_ack: false,
        };
        let outcome = forwarder.deliver(&fwd).await;
        assert_eq!(outcome.delivered_to, 0);
        assert_eq!(outcome.failed, 0);
        assert!(!outcome.acked_back);
    }

    #[tokio::test]
    async fn deliver_with_local_subscriber_succeeds_and_marks_ack_required() {
        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        let client = dummy_client_with_id(0x42u128);
        registry.add_client(client.clone());
        registry.register_title(0x42u128, "delivered_chan");

        let slot = Arc::new(ArcSwap::from_pointee(None));
        let route_table = local_routing_table();
        let forwarder = NetworkForwarder::new(
            slot,
            route_table,
            rex_cluster::types::NodeId::new("local-node"),
            registry,
        );

        let rex_data =
            rex_core::RexData::new(rex_core::RexCommand::Title, "delivered_chan", b"hello");
        let fwd = ForwardMessage {
            forward_id: 7,
            original_source: 0xAAu128,
            target_client_id: 0,
            title: "delivered_chan".into(),
            payload: rex_data.pack_ref().to_vec(),
            is_group: false,
            is_broadcast: false,
            require_ack: true,
        };

        let outcome = forwarder.deliver(&fwd).await;
        assert_eq!(outcome.delivered_to, 1);
        assert_eq!(outcome.failed, 0);
        assert!(outcome.acked_back, "acked_back should mirror require_ack");
    }

    #[tokio::test]
    async fn deliver_broadcast_delivers_to_all_subscribers() {
        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        for id in [0x10u128, 0x20u128, 0x30u128] {
            let c = dummy_client_with_id(id);
            registry.add_client(c.clone());
            registry.register_title(id, "fanout_chan");
        }

        let slot = Arc::new(ArcSwap::from_pointee(None));
        let route_table = local_routing_table();
        let forwarder = NetworkForwarder::new(
            slot,
            route_table,
            rex_cluster::types::NodeId::new("local-node"),
            registry,
        );

        let rex_data =
            rex_core::RexData::new(rex_core::RexCommand::Title, "fanout_chan", b"hi-all");
        let fwd = ForwardMessage {
            forward_id: 11,
            original_source: 0xAAu128,
            target_client_id: 0,
            title: "fanout_chan".into(),
            payload: rex_data.pack_ref().to_vec(),
            is_group: false,
            is_broadcast: true,
            require_ack: false,
        };

        let outcome = forwarder.deliver(&fwd).await;
        assert_eq!(outcome.delivered_to, 3);
        assert_eq!(outcome.failed, 0);
        assert!(!outcome.acked_back);
    }

    #[tokio::test]
    async fn deliver_with_unknown_client_does_not_panic() {
        let forwarder = empty_forwarder();
        let rex_data = rex_core::RexData::new(rex_core::RexCommand::Title, "absent", b"hello");
        let fwd = ForwardMessage {
            forward_id: 12,
            original_source: 0xAAu128,
            target_client_id: 0xDEADu128,
            title: "absent".into(),
            payload: rex_data.pack_ref().to_vec(),
            is_group: false,
            is_broadcast: false,
            require_ack: false,
        };
        let outcome = forwarder.deliver(&fwd).await;
        assert_eq!(outcome.delivered_to, 0);
    }

    #[test]
    fn fwd_result_variants_distinct() {
        // Each variant must be a distinct failure mode so handlers
        // can branch on intent.
        let variants = [
            FwdResult::Delivered,
            FwdResult::NoPeerForTitle,
            FwdResult::PeerUnreachable("a".into()),
            FwdResult::PeerRejected("b".into()),
        ];
        for (i, a) in variants.iter().enumerate() {
            for (j, b) in variants.iter().enumerate() {
                if i != j {
                    assert_ne!(a, b);
                }
            }
        }
    }

    #[test]
    fn delivery_outcome_default_is_zero() {
        let o = DeliveryOutcome::default();
        assert_eq!(o.delivered_to, 0);
        assert_eq!(o.failed, 0);
        assert!(!o.acked_back);
    }
}
