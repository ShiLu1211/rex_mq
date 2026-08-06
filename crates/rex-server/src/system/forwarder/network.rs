//! Production [`Forwarder`] impl for `NetworkForwarder`, plus the
//! private `try_send` / `try_reconnect_and_send` helpers that drive
//! the cluster wire send with a single retry on disconnect.

use std::net::SocketAddr;
use std::sync::Arc;

use async_trait::async_trait;
use bytes::BytesMut;
use rex_cluster::node::NodeManager;
use rex_cluster::transport::ClusterTransport;
use rex_cluster::types::{ClusterMessage, ForwardAckMessage, ForwardMessage};
use rex_core::RexData;
use tracing::{debug, info, warn};

use crate::ForwardRequest;
use crate::system::forwarder::{DeliveryOutcome, Forwarder, FwdResult, NetworkForwarder};

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

impl NetworkForwarder {
    /// Walk all known connected peers (excluding `exclude` and self),
    /// trying each until one accepts. Returns `Delivered` on the first
    /// success; `NoPeerForTitle` if no other known peer has an
    /// address; `PeerUnreachable(excluded)` if every other peer also
    /// failed.
    pub(super) async fn fallback_excluding(
        &self,
        req: &ForwardRequest,
        exclude: Option<&str>,
    ) -> FwdResult {
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

/// Send `req` over the cluster transport to `target_node` at `addr`.
/// Returns `true` if the transport accepted the message. On failure,
/// attempts a single reconnect-and-retry before giving up.
pub(super) async fn try_send(
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
    transport: &Arc<ClusterTransport>,
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
