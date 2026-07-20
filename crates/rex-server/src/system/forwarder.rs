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
use rex_cluster::types::{ClusterMessage, ForwardAckMessage, ForwardMessage};
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

    /// Broadcast a `ForwardAck` to every connected peer. Used by
    /// the dispatch loop after `Forwarder::deliver` returns
    /// `acked_back: true` for a `require_ack` message. ADR-0003
    /// records this is the only entry point for ack transmission.
    async fn announce_ack(&self, ack: &ForwardAckMessage);

    /// Cluster-internal fan-out (e.g. `TitleRegister`).
    /// Returns the number of sends the transport accepted (best-effort,
    /// may be `0` if the cluster is not started).
    async fn broadcast(&self, msg: &ClusterMessage) -> usize;

    /// True iff the cluster is started (the `NodeManager` slot is
    /// populated). Used by the observability `/readyz` forwarder probe.
    /// Default returns `false` so older / mock implementations stay
    /// compatible.
    fn is_cluster_started(&self) -> bool {
        false
    }
}

/// Production [`Forwarder`] implementation.
///
/// Holds two `ArcSwap` slots — empty at construction, populated by
/// `ServerClusterManager::start()`:
/// - `node_manager` — the cluster's `NodeManager` (gives us the
///   transport for outbound sends).
/// - `route_table` — the cluster's `GlobalRouteTable` (peer lookup).
///
/// Reads use `ArcSwap::load` for an async-lock-free hot path.
pub struct NetworkForwarder {
    node_manager: Arc<ArcSwap<Option<Arc<NodeManager>>>>,
    route_table: Arc<ArcSwap<Option<Arc<GlobalRouteTable>>>>,
    local_node_id: rex_cluster::types::NodeId,
    client_registry: Arc<dyn ClientRegistry>,
}

impl NetworkForwarder {
    /// Construct a new `NetworkForwarder`. Both slots are empty —
    /// populate them via [`NetworkForwarder::set_node_manager`] and
    /// [`NetworkForwarder::set_route_table`] once the cluster has
    /// started. Until then, `forward` returns
    /// `PeerUnreachable("cluster-not-started")`.
    pub fn new(
        node_manager: Arc<ArcSwap<Option<Arc<NodeManager>>>>,
        route_table: Arc<ArcSwap<Option<Arc<GlobalRouteTable>>>>,
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

    /// Populate the cluster's `GlobalRouteTable` slot. Same
    /// lifecycle as `set_node_manager`.
    pub fn set_route_table(&self, rt: Option<Arc<GlobalRouteTable>>) {
        self.route_table.store(Arc::new(rt));
    }

    /// Load the current node manager, if any. The hot path on
    /// `forward` uses this directly.
    fn node_manager(&self) -> Option<Arc<NodeManager>> {
        // `load` returns the inner `Arc<Option<Arc<NodeManager>>>`;
        // `as_ref().clone()` peels one Arc and clones the Option.
        self.node_manager.load().as_ref().clone()
    }

    /// Load the current route table, if any. The hot path on
    /// `forward` uses this directly.
    fn route_table(&self) -> Option<Arc<GlobalRouteTable>> {
        self.route_table.load().as_ref().clone()
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
        let route_table = match self.route_table() {
            Some(rt) => rt,
            None => return FwdResult::NoPeerForTitle,
        };
        let transport = nm.get_transport();
        let local_id = self.local_node_id.to_string();

        for other in transport.connected_nodes() {
            if other == local_id || Some(other.as_str()) == exclude {
                continue;
            }
            let Some(addr_str) = route_table.get_node_addr(&other) else {
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
        let route_table = match self.route_table() {
            Some(rt) => rt,
            None => return FwdResult::PeerUnreachable("cluster-not-started".into()),
        };

        // 2. Look up the targeted peer's address. Unknown peer →
        //    try the fallback walk.
        let Some(addr_str) = route_table.get_node_addr(target_node) else {
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

    async fn announce_ack(&self, ack: &ForwardAckMessage) {
        let Some(nm) = self.node_manager() else {
            return;
        };
        let transport = nm.get_transport();
        let local_id = self.local_node_id.to_string();
        let message = ClusterMessage::ForwardAck(ack.clone());

        for node_id in transport.connected_nodes() {
            if node_id == local_id {
                continue;
            }
            if let Err(e) = transport.send_to(&node_id, &message).await {
                warn!("Failed to broadcast forward ack to node {}: {}", node_id, e);
            }
        }
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

    fn is_cluster_started(&self) -> bool {
        // `ArcSwap::load` returns `Arc<Option<Arc<NodeManager>>>`;
        // `is_some` only checks the Option — the inner `Arc` is just
        // a wrapper around the populated slot.
        self.node_manager.load().is_some()
    }
}

/// Send `req` over the cluster transport to `target_node` at `addr`.
/// Returns `true` if the transport accepted the message. On failure,
/// attempts a single reconnect-and-retry before giving up.
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

    fn empty_forwarder() -> Arc<NetworkForwarder> {
        let nm_slot = Arc::new(ArcSwap::from_pointee(None));
        let rt_slot = Arc::new(ArcSwap::from_pointee(None));
        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        NetworkForwarder::new(
            nm_slot,
            rt_slot,
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
    async fn announce_ack_with_transport_walks_connected_nodes() -> anyhow::Result<()> {
        let (peer_a_addr, _listener_a, mut received_a) = recording_listener().await?;
        let (peer_b_addr, _listener_b, mut received_b) = recording_listener().await?;
        let forwarder = started_forwarder(
            "local-node",
            &[("node-a", peer_a_addr, true), ("node-b", peer_b_addr, true)],
        )
        .await;
        let ack = ForwardAckMessage {
            forward_id: 1,
            from_node_id: "local-node".into(),
            original_source: 0xAAu128,
            success: true,
            error: None,
        };

        forwarder.announce_ack(&ack).await;

        // Decode the framed payload (4-byte big-endian length header is
        // already stripped by `recording_listener`) and confirm both
        // connected peers received the same `ForwardAck` we broadcast.
        // The first frame after `transport.connect` is the
        // `ForwardAck` itself — there is no application-level handshake
        // in the current transport — but decoding instead of counting
        // bytes guards against a future handshake frame sneaking in.
        let payload_a = timeout(Duration::from_secs(1), received_a.recv())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for node-a ack"))?
            .ok_or_else(|| anyhow::anyhow!("node-a listener closed before receiving ack"))?;
        let payload_b = timeout(Duration::from_secs(1), received_b.recv())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for node-b ack"))?
            .ok_or_else(|| anyhow::anyhow!("node-b listener closed before receiving ack"))?;
        let msg_a: ClusterMessage = bincode::deserialize(&payload_a)?;
        let msg_b: ClusterMessage = bincode::deserialize(&payload_b)?;
        match msg_a {
            ClusterMessage::ForwardAck(got) => {
                assert_eq!(got.forward_id, ack.forward_id, "node-a ack.forward_id");
                assert_eq!(
                    got.from_node_id, ack.from_node_id,
                    "node-a ack.from_node_id"
                );
                assert_eq!(
                    got.original_source, ack.original_source,
                    "node-a ack.original_source"
                );
                assert_eq!(got.success, ack.success, "node-a ack.success");
                assert_eq!(got.error, ack.error, "node-a ack.error");
            }
            other => panic!("node-a expected ClusterMessage::ForwardAck, got {other:?}"),
        }
        match msg_b {
            ClusterMessage::ForwardAck(got) => {
                assert_eq!(got.forward_id, ack.forward_id, "node-b ack.forward_id");
                assert_eq!(
                    got.from_node_id, ack.from_node_id,
                    "node-b ack.from_node_id"
                );
                assert_eq!(
                    got.original_source, ack.original_source,
                    "node-b ack.original_source"
                );
                assert_eq!(got.success, ack.success, "node-b ack.success");
                assert_eq!(got.error, ack.error, "node-b ack.error");
            }
            other => panic!("node-b expected ClusterMessage::ForwardAck, got {other:?}"),
        }
        Ok(())
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

        let nm_slot = Arc::new(ArcSwap::from_pointee(None));
        let rt_slot = Arc::new(ArcSwap::from_pointee(None));
        let forwarder = NetworkForwarder::new(
            nm_slot,
            rt_slot,
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

        let nm_slot = Arc::new(ArcSwap::from_pointee(None));
        let rt_slot = Arc::new(ArcSwap::from_pointee(None));
        let forwarder = NetworkForwarder::new(
            nm_slot,
            rt_slot,
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

    // ---------- Helpers for started-cluster tests ----------
    //
    // The `Forwarder::forward` contract requires a populated cluster
    // (`node_manager` + `route_table`). Spinning up real cluster
    // servers would be overkill for unit tests, so we wire a real
    // `NodeManager` + `ClusterTransport` against a local TCP listener
    // that just drains incoming bytes. The listener accepts the TCP
    // connection; `ClusterTransport::connect` inserts the sender into
    // its connection map before the wire handshake completes, so
    // `is_connected` returns true and `send_to` writes through the
    // local channel regardless of what the listener does.

    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;
    use tokio::time::{Duration, timeout};

    /// Bind a TCP listener on `127.0.0.1:0` and accept connections in
    /// the background, draining incoming bytes until the peer closes.
    /// Returns the bound address and a handle for abort on drop.
    async fn drain_listener() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind drain listener");
        let addr = listener.local_addr().expect("listener local_addr");
        let handle = tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => continue,
                        }
                    }
                });
            }
        });
        (addr, handle)
    }

    /// Bind a TCP listener that records each framed cluster message
    /// (4-byte big-endian length header already stripped) and forwards
    /// the raw payload to the caller via an unbounded mpsc channel.
    /// Used to verify that fan-out paths reach every connected peer.
    async fn recording_listener() -> anyhow::Result<(
        SocketAddr,
        tokio::task::JoinHandle<()>,
        tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    )> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            loop {
                let (mut stream, _) = match listener.accept().await {
                    Ok(pair) => pair,
                    Err(_) => break,
                };
                let message_tx = message_tx.clone();
                tokio::spawn(async move {
                    loop {
                        let mut length = [0u8; 4];
                        if stream.read_exact(&mut length).await.is_err() {
                            break;
                        }
                        let mut payload = vec![0u8; u32::from_be_bytes(length) as usize];
                        if stream.read_exact(&mut payload).await.is_err() {
                            break;
                        }
                        let _ = message_tx.send(payload);
                    }
                });
            }
        });
        Ok((addr, handle, message_rx))
    }

    /// Build a started `NetworkForwarder` with a real
    /// `NodeManager` + `ClusterTransport` + `GlobalRouteTable`. The
    /// caller supplies a list of `(peer_id, peer_addr, should_connect)`
    /// tuples. Each peer is registered in the route table; only the
    /// peers with `should_connect = true` are wired into the transport
    /// connection map. Returns the populated forwarder.
    async fn started_forwarder(
        local_id: &str,
        peers: &[(&str, SocketAddr, bool)],
    ) -> Arc<NetworkForwarder> {
        let nm_slot = Arc::new(ArcSwap::from_pointee(None));
        let rt_slot = Arc::new(ArcSwap::from_pointee(None));
        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        let forwarder = NetworkForwarder::new(
            nm_slot.clone(),
            rt_slot.clone(),
            rex_cluster::types::NodeId::new(local_id),
            registry,
        );

        // NodeManager requires a ClusterMessage channel for inbound
        // forward; the receiver is dropped immediately so any incoming
        // bytes are silently discarded (matches the unit-test pattern
        // used elsewhere in this module).
        let (message_tx, _message_rx) =
            tokio::sync::mpsc::unbounded_channel::<rex_cluster::types::ClusterMessage>();
        let config = rex_cluster::types::ClusterConfig::new(
            rex_cluster::types::NodeId::new(local_id),
            "127.0.0.1:0".parse().expect("parse local listen addr"),
        );
        let nm = Arc::new(rex_cluster::node::NodeManager::new(config, message_tx));
        let transport = nm.get_transport();

        let route_table = rex_cluster::route_table::GlobalRouteTable::with_local_node(
            rex_cluster::types::NodeId::new(local_id),
        );
        for (id, addr, should_connect) in peers {
            route_table.add_node((*id).to_string(), addr.to_string());
            if *should_connect {
                transport
                    .connect(rex_cluster::types::NodeId::new(*id), *addr)
                    .await
                    .expect("transport.connect");
            }
        }

        forwarder.set_node_manager(Some(nm));
        forwarder.set_route_table(Some(Arc::new(route_table)));
        forwarder
    }

    // ---------- forward() direct-send + fallback tests ----------

    /// Direct send to a known peer whose address is in the route
    /// table. The transport accepts the message, so `forward` returns
    /// `Delivered` without walking the fallback list. Guards against
    /// regressing the fallback loop (e.g. by triggering it on every
    /// successful send).
    ///
    /// Two peers are wired so the fallback walk has at least one
    /// candidate. The test asserts not only that the targeted peer
    /// (node-b) receives a frame, but also that the other connected
    /// peer (node-c) receives nothing — proving the fallback walk was
    /// not entered at all. A single-peer wiring would pass even if the
    /// fallback walk regressed, because the walk would have no
    /// candidate to attempt.
    #[tokio::test]
    async fn forward_to_known_target_accepted_does_not_fallback() -> anyhow::Result<()> {
        let (peer_b_addr, _listener_b, mut received_b) = recording_listener().await?;
        let (peer_c_addr, _listener_c, mut received_c) = recording_listener().await?;
        let fwd = started_forwarder(
            "local-node",
            &[("node-b", peer_b_addr, true), ("node-c", peer_c_addr, true)],
        )
        .await;
        let req = sample_forward_request("news");

        let r = fwd.forward("node-b", &req).await;
        assert!(matches!(r, FwdResult::Delivered), "got {r:?}");

        // Targeted peer must have received exactly one framed payload.
        let payload_b = timeout(Duration::from_secs(1), received_b.recv())
            .await
            .map_err(|_| anyhow::anyhow!("timed out waiting for node-b frame"))?
            .ok_or_else(|| anyhow::anyhow!("node-b listener closed before receiving frame"))?;
        let msg_b: ClusterMessage = bincode::deserialize(&payload_b)?;
        match msg_b {
            ClusterMessage::Forward(_) => {}
            other => panic!("node-b expected ClusterMessage::Forward, got {other:?}"),
        }

        // The fallback walk must not have attempted node-c. Use a
        // short timeout — the absence of a frame is the assertion.
        let frame_c = timeout(Duration::from_millis(100), received_c.recv()).await;
        assert!(
            frame_c.is_err(),
            "node-c received an unexpected frame (fallback walk entered): {frame_c:?}"
        );
        Ok(())
    }

    /// When the targeted peer is registered in the route table but
    /// has no live transport connection, the direct path fails and
    /// `forward` must walk the other connected peers. Today `title.rs`
    /// hand-rolls this loop; this test guards `Forwarder::forward`
    /// from regressing without it.
    #[tokio::test]
    async fn forward_target_refused_falls_back_to_other_peer() {
        // node-b is registered in the route table but NEVER connected
        // (so `is_connected("node-b")` is false and the direct send
        // fails). node-c is connected and accepts the message; the
        // fallback walk should land on it.
        let (peer_c_addr, _listener_c) = drain_listener().await;
        let unreachable: SocketAddr = "127.0.0.1:1".parse().expect("parse unreachable");
        let fwd = started_forwarder(
            "local-node",
            &[
                ("node-b", unreachable, false),
                ("node-c", peer_c_addr, true),
            ],
        )
        .await;
        let req = sample_forward_request("news");

        let r = fwd.forward("node-b", &req).await;
        assert!(matches!(r, FwdResult::Delivered), "got {r:?}");
    }
}
