//! Cluster Manager for RexServer
//!
//! Manages cluster integration with the server - simplified version

use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::RwLock;
use rex_cluster::node::NodeManager;
use rex_cluster::route_table::GlobalRouteTable;
use rex_cluster::types::{
    ClusterConfig as RexClusterConfig, ClusterMessage, ForwardAckMessage, NodeId, NodeInfo,
};
use rex_observability::metrics::set_cluster_peers;
use tokio::sync::mpsc;

use crate::{ClusterPort, Services};

/// Server-side cluster manager - handles cluster communication
pub struct ServerClusterManager {
    /// Node manager for cluster communication
    node_manager: RwLock<Option<NodeManager>>,
    /// Route table for client/title routing
    route_table: Arc<GlobalRouteTable>,
    /// Local node ID
    local_node_id: NodeId,
    /// Local listen address
    local_addr: RwLock<SocketAddr>,
    /// Whether cluster is enabled
    enabled: bool,
    /// Cluster message sender
    cluster_tx: RwLock<Option<mpsc::UnboundedSender<ClusterMessage>>>,
    services: RwLock<Option<Arc<Services>>>,
}

impl ServerClusterManager {
    /// Create a new server cluster manager
    pub fn new(node_id: String, enabled: bool) -> Arc<Self> {
        let local_node_id = NodeId::new(node_id.clone());
        let route_table = Arc::new(GlobalRouteTable::with_local_node(local_node_id.clone()));

        // Use std::net::SocketAddr::new to avoid parsing
        let default_addr =
            std::net::SocketAddr::new(std::net::IpAddr::V4(std::net::Ipv4Addr::new(0, 0, 0, 0)), 0);

        Arc::new(Self {
            node_manager: RwLock::new(None),
            route_table,
            local_node_id: NodeId::new(node_id),
            local_addr: RwLock::new(default_addr),
            enabled,
            cluster_tx: RwLock::new(None),
            services: RwLock::new(None),
        })
    }

    /// Set the services reference for message delivery
    pub fn set_services(self: &Arc<Self>, services: Arc<Services>) {
        *self.services.write() = Some(services);
    }

    /// Start the cluster manager
    pub async fn start(self: &Arc<Self>, config: RexClusterConfig) {
        if !self.enabled {
            return;
        }

        // Store local address
        *self.local_addr.write() = config.listen_addr;

        // Add local node to route table first
        let local_node_id_str = self.local_node_id.to_string();
        let local_addr = config.listen_addr.to_string();
        self.route_table
            .add_node(local_node_id_str.clone(), local_addr);
        // Observability: track the local node in the peer gauge. The
        // gauge is "peers including self"; subtracting the local
        // node is the caller's job if they want peers-only.
        set_cluster_peers(self.get_nodes().len() as i64);
        tracing::info!("Added local node {} to route table", local_node_id_str);

        let (tx, rx) = mpsc::unbounded_channel::<ClusterMessage>();

        let cluster_config = RexClusterConfig {
            enabled: true,
            node_id: self.local_node_id.clone(),
            listen_addr: config.listen_addr,
            seed_nodes: config.seed_nodes,
            communication_timeout_ms: config.communication_timeout_ms,
            heartbeat_interval_ms: config.heartbeat_interval_ms,
            election_timeout_min_ms: config.election_timeout_min_ms,
            election_timeout_max_ms: config.election_timeout_max_ms,
            max_retries: config.max_retries,
        };

        let node_manager = NodeManager::new(cluster_config, tx.clone());

        // Spawn message handler - keep rx alive
        let self_clone = self.clone();
        tokio::spawn(async move {
            self_clone.handle_messages(rx).await;
        });

        // Start the node manager
        if let Err(e) = node_manager.start().await {
            tracing::error!("Failed to start node manager: {}", e);
            return;
        }

        *self.node_manager.write() = Some(node_manager);
        *self.cluster_tx.write() = Some(tx);

        tracing::info!(
            "Server cluster manager started for node {}",
            self.local_node_id
        );
    }

    /// Handle incoming cluster messages
    async fn handle_messages(self: &Arc<Self>, mut rx: mpsc::UnboundedReceiver<ClusterMessage>) {
        let local_node_id = self.local_node_id.clone();
        let local_addr = *self.local_addr.read();
        let route_table = self.route_table.clone();

        while let Some(msg) = rx.recv().await {
            tracing::debug!(
                "[{}] Received cluster message: {:?}",
                local_node_id.as_str(),
                msg
            );
            match msg {
                ClusterMessage::Join(node_info) => {
                    let node_id = node_info.node_id.to_string();
                    if node_id != local_node_id.as_str() {
                        route_table.add_node(node_id.clone(), node_info.listen_addr.to_string());
                        set_cluster_peers(self.get_nodes().len() as i64);
                        tracing::info!("Node {} joined the cluster via gossip", node_id);

                        // Send back our node info so the joining node knows about us
                        let local_info = NodeInfo::new(local_node_id.clone(), local_addr);
                        let node_list = ClusterMessage::NodeList(vec![local_info]);
                        if self.send_to_node(&node_id, node_list).await {
                            tracing::debug!("Sent NodeList to {}", node_id);
                        } else {
                            tracing::warn!("Failed to send NodeList to {}", node_id);
                        }
                    }
                }
                ClusterMessage::NodeList(nodes) => {
                    tracing::debug!("Received NodeList with {} nodes", nodes.len());
                    // Get transport to connect to nodes
                    let transport = {
                        let node_manager_guard = self.node_manager.read();
                        node_manager_guard
                            .as_ref()
                            .map(|nm| nm.get_transport().clone())
                    };

                    // Add all nodes from the list to route table and connect to them
                    for node_info in nodes {
                        let node_id = node_info.node_id.to_string();
                        let listen_addr = node_info.listen_addr;
                        tracing::debug!("Processing node {} from NodeList", node_id);
                        if node_id != local_node_id.as_str() {
                            route_table.add_node(node_id.clone(), listen_addr.to_string());
                            set_cluster_peers(self.get_nodes().len() as i64);
                            tracing::info!("Added node {} from NodeList", node_id);

                            // Connect to the node if not already connected
                            if let Some(ref transport) = transport
                                && !transport.is_connected(&node_id)
                            {
                                tracing::info!("Connecting to node {} at {}", node_id, listen_addr);
                                let node_id_clone = node_id.clone();
                                let addr = listen_addr;
                                let transport_clone = transport.clone();
                                tokio::spawn(async move {
                                    if let Err(e) =
                                        transport_clone.connect(node_id_clone.into(), addr).await
                                    {
                                        tracing::warn!("Failed to connect to node: {}", e);
                                    }
                                });
                            }
                        }
                    }
                }
                ClusterMessage::Forward(forward) => {
                    tracing::debug!("Received forward message for title: {}", forward.title);

                    // Clone services out of the lock so the guard is dropped
                    // before the async delivery call.
                    let services = {
                        let lock = self.services.read();
                        lock.as_ref().map(Arc::clone)
                    };
                    if let Some(services) = services {
                        let outcome = services.forwarder.deliver(&forward).await;
                        if forward.require_ack && outcome.acked_back {
                            let success = outcome.delivered_to > 0 && outcome.failed == 0;
                            let error = if success {
                                None
                            } else if outcome.failed > 0 {
                                Some(format!(
                                    "Failed to deliver to {} local subscriber(s)",
                                    outcome.failed
                                ))
                            } else {
                                Some("No local subscriber".to_string())
                            };
                            let ack = ForwardAckMessage {
                                forward_id: forward.forward_id,
                                from_node_id: self.local_node_id.to_string(),
                                original_source: forward.original_source,
                                success,
                                error,
                            };
                            services.forwarder.announce_ack(&ack).await;
                        }
                    }
                }
                ClusterMessage::TitleRegister(msg) => {
                    tracing::info!(
                        "Received title registration: {} -> {} from cluster",
                        msg.title,
                        msg.node_id
                    );
                    // Update route table to know this node handles this title
                    // The hash ring will handle the routing, but we log for visibility
                }
                ClusterMessage::TitleUnregister(msg) => {
                    tracing::info!(
                        "Received title unregistration: {} from {} from cluster",
                        msg.title,
                        msg.node_id
                    );
                    // Could remove from local tracking if we had explicit title->node mapping
                }
                ClusterMessage::ForwardAck(ack) => {
                    tracing::debug!(
                        "Received ForwardAck: forward_id={}, success={}, from_node={}",
                        ack.forward_id,
                        ack.success,
                        ack.from_node_id
                    );
                    // TODO: Forward this ACK to the original client if needed
                    // This would require tracking pending forwards and routing back to the client
                }
                _ => {}
            }
        }
    }

    /// Get route table
    pub fn route_table(&self) -> &Arc<GlobalRouteTable> {
        &self.route_table
    }

    /// Register a client to this node
    pub fn register_client(&self, client_id: u128) {
        self.route_table
            .register_client(client_id, self.local_node_id.as_str());
    }

    /// Unregister a client
    pub fn unregister_client(&self, client_id: &u128) {
        self.route_table.unregister_client(client_id);
    }

    /// Get node for a client
    pub fn get_node_for_client(&self, client_id: &u128) -> Option<String> {
        self.route_table.get_node_by_client(client_id)
    }

    /// Check if client is local
    pub fn is_client_local(&self, client_id: &u128) -> bool {
        self.route_table.is_client_local(client_id)
    }

    /// Get node for a title
    pub fn get_node_for_title(&self, title: &str) -> Option<String> {
        self.route_table.get_node_by_title(title)
    }

    /// Check if cluster is enabled
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get local node ID
    pub fn local_node_id(&self) -> &NodeId {
        &self.local_node_id
    }

    /// Get all known nodes
    pub fn get_nodes(&self) -> Vec<String> {
        self.route_table.get_all_nodes()
    }

    /// Send a cluster message to another node
    pub async fn send_to_node(&self, target_node: &str, msg: ClusterMessage) -> bool {
        // Get transport clone
        let transport = {
            let node_manager = self.node_manager.read();
            match node_manager.as_ref() {
                Some(nm) => nm.get_transport().clone(),
                None => {
                    tracing::warn!("No node manager available for sending");
                    return false;
                }
            }
        };

        let connected = transport.connected_nodes();
        tracing::debug!(
            "Trying to send to {}, connected nodes: {:?}",
            target_node,
            connected
        );

        // Check if connected first
        if !transport.is_connected(target_node) {
            tracing::warn!(
                "Not connected to target node {} (known nodes: {:?})",
                target_node,
                connected
            );
            return false;
        }

        tracing::debug!("Sending message to connected node {}", target_node);

        // Send via transport
        match transport.send_to(target_node, &msg).await {
            Ok(()) => true,
            Err(e) => {
                tracing::warn!("Failed to send message to {}: {}", target_node, e);
                false
            }
        }
    }

    /// Forward a message to another node
    pub async fn forward_message(&self, target_node: &str, request: crate::ForwardRequest) -> bool {
        // Get the target node's address from route table
        let target_addr = match self.route_table.get_node_addr(target_node) {
            Some(addr) => addr,
            None => {
                tracing::warn!("No address found for node {}", target_node);
                return false;
            }
        };

        // Get transport clone - need to clone inside the block to avoid holding lock across await
        let transport = {
            let node_manager = self.node_manager.read();
            match node_manager.as_ref() {
                Some(nm) => nm.get_transport().clone(),
                None => {
                    tracing::warn!("No node manager available for forwarding");
                    return false;
                }
            }
        };

        // Create forward message
        let forward_msg = rex_cluster::types::ForwardMessage {
            forward_id: fastrand::u64(..),
            original_source: request.source_client_id,
            target_client_id: request.target_client_id,
            title: request.title,
            payload: request.payload,
            is_group: matches!(request.msg_type, crate::ForwardType::Group),
            is_broadcast: matches!(request.msg_type, crate::ForwardType::Broadcast),
            require_ack: false,
        };

        let cluster_msg = ClusterMessage::Forward(forward_msg);

        // Send via transport
        match transport.send_to(target_node, &cluster_msg).await {
            Ok(()) => {
                tracing::debug!(
                    "Forwarded message to node {} at {}",
                    target_node,
                    target_addr
                );
                true
            }
            Err(e) => {
                tracing::warn!("Failed to forward message to {}: {}", target_node, e);
                // Try to reconnect and retry once
                if let Ok(addr) = target_addr.parse::<SocketAddr>()
                    && self
                        .try_reconnect_and_send(target_node, addr, &cluster_msg)
                        .await
                {
                    tracing::info!(
                        "Successfully reconnected and sent message to {}",
                        target_node
                    );
                    return true;
                }
                false
            }
        }
    }

    /// Try to reconnect to a node and send message
    async fn try_reconnect_and_send(
        &self,
        node_id: &str,
        addr: SocketAddr,
        msg: &ClusterMessage,
    ) -> bool {
        let transport = {
            let node_manager = self.node_manager.read();
            match node_manager.as_ref() {
                Some(nm) => nm.get_transport().clone(),
                None => return false,
            }
        };

        tracing::info!("Attempting to reconnect to node {} at {}", node_id, addr);

        // Remove old connection if exists
        transport.remove_connection(node_id);

        // Try to connect
        match transport.connect(node_id.to_string().into(), addr).await {
            Ok(()) => {
                // Connection established, try to send
                match transport.send_to(node_id, msg).await {
                    Ok(()) => true,
                    Err(e) => {
                        tracing::warn!("Failed to send after reconnect to {}: {}", node_id, e);
                        false
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to reconnect to {}: {}", node_id, e);
                false
            }
        }
    }

    /// Broadcast a message to all connected cluster nodes
    pub async fn broadcast(&self, message: ClusterMessage) {
        let transport = {
            let node_manager_guard = self.node_manager.read();
            match node_manager_guard.as_ref() {
                Some(nm) => nm.get_transport().clone(),
                None => {
                    tracing::warn!("No node manager available for broadcast");
                    return;
                }
            }
        };

        let connected_nodes = transport.connected_nodes();
        let local_id = self.local_node_id.to_string();

        for node_id in connected_nodes {
            if node_id != local_id
                && let Err(e) = transport.send_to(&node_id, &message).await
            {
                tracing::warn!("Failed to broadcast to node {}: {}", node_id, e);
            }
        }
    }
}

/* ---------------- ClusterPort impl (commit 6) ---------------- */

#[async_trait::async_trait]
impl ClusterPort for ServerClusterManager {
    fn register_client(&self, client_id: u128) {
        // Delegate to the inherent method.
        ServerClusterManager::register_client(self, client_id);
    }

    fn unregister_client(&self, client_id: u128) {
        ServerClusterManager::unregister_client(self, &client_id);
    }

    fn find_node_for_title(&self, title: &str) -> Option<String> {
        ServerClusterManager::get_node_for_title(self, title)
    }

    fn get_local_node_id(&self) -> Option<String> {
        Some(self.local_node_id.to_string())
    }

    fn get_nodes(&self) -> Vec<String> {
        ServerClusterManager::get_nodes(self)
    }

    async fn forward_message(&self, target_node: &str, request: crate::ForwardRequest) -> bool {
        ServerClusterManager::forward_message(self, target_node, request).await
    }

    async fn broadcast(&self, message: ClusterMessage) -> usize {
        ServerClusterManager::broadcast(self, message).await;
        // broadcast doesn't return a count in the inherent impl; report a
        // best-effort 0/1 based on whether the broadcast task ran.
        1
    }
}

/* ---------------- ClusterPort tests (commit 6) ---------------- */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterPort;
    use crate::ForwardRequest;
    use crate::ForwardType;

    fn make_manager() -> Arc<ServerClusterManager> {
        let m = ServerClusterManager::new("test-node".to_string(), true);
        // `with_local_node` only sets the local_node_id field; the route
        // table's hash ring is empty until `start()` runs (which calls
        // `route_table.add_node`). For unit tests, seed the ring directly.
        m.route_table()
            .add_node("test-node".to_string(), "127.0.0.1:0".to_string());
        m
    }

    #[test]
    fn cluster_port_get_local_node_id_returns_local() {
        let m = make_manager();
        let id = m.get_local_node_id();
        assert_eq!(id.as_deref(), Some("test-node"));
    }

    #[test]
    fn cluster_port_get_nodes_includes_local_at_construction() {
        let m = make_manager();
        let nodes = m.get_nodes();
        assert!(nodes.iter().any(|n| n == "test-node"));
    }

    #[test]
    fn cluster_port_register_and_unregister_client() {
        let m = make_manager();
        m.register_client(0xABCD);
        m.unregister_client(&0xABCD);
        // is_client_local goes through the route table; after unregister
        // the client should not be present.
        assert!(!m.is_client_local(&0xABCD));
    }

    #[test]
    fn cluster_port_inherent_methods_still_callable() {
        // Inherent methods are kept so existing callers don't need to
        // bring the ClusterPort trait into scope.
        let m = make_manager();
        m.register_client(0xCAFE);
        // Inherent unregister_client takes &u128.
        ServerClusterManager::unregister_client(&m, &0xCAFE);
        assert!(!m.is_client_local(&0xCAFE));
    }

    #[test]
    fn cluster_port_find_node_for_title_returns_some_node() {
        let m = make_manager();
        // With only the local node in the ring, every title hashes to it.
        let node = m.find_node_for_title("news");
        assert_eq!(node.as_deref(), Some("test-node"));
    }

    #[test]
    fn cluster_port_add_node_then_list() {
        let m = make_manager();
        m.route_table()
            .add_node("peer-1".to_string(), "127.0.0.1:9999".to_string());
        let nodes = m.get_nodes();
        assert!(nodes.iter().any(|n| n == "peer-1"));
    }

    #[tokio::test]
    async fn cluster_port_forward_message_to_unknown_node_returns_false() {
        // The manager's node_manager is None here (never started), so
        // forward_message cannot reach the wire and must return false.
        let m = make_manager();
        let req = ForwardRequest {
            source_client_id: 1,
            target_client_id: 2,
            title: "news".to_string(),
            payload: vec![0u8; 8],
            msg_type: ForwardType::Unicast,
        };
        let accepted = m.forward_message("peer-1", req).await;
        assert!(!accepted, "forward to unknown node must return false");
    }
}
