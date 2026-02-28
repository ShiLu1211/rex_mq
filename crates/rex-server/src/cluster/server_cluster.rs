//! Cluster Manager for RexServer
//!
//! Manages cluster integration with the server - simplified version

use std::net::SocketAddr;
use std::sync::Arc;

use parking_lot::RwLock;
use rex_cluster::node::NodeManager;
use rex_cluster::route_table::GlobalRouteTable;
use rex_cluster::types::{ClusterConfig as RexClusterConfig, ClusterMessage, NodeId, NodeInfo};
use rex_core::RexData;
use tokio::sync::mpsc;

use crate::RexSystem;

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
    /// Reference to the system for message delivery
    system: RwLock<Option<Arc<RexSystem>>>,
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
            system: RwLock::new(None),
        })
    }

    /// Set the system reference for message delivery
    pub fn set_system(self: &Arc<Self>, system: Arc<RexSystem>) {
        *self.system.write() = Some(system);
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

                    // Deliver the message locally
                    self.deliver_forward_message(forward).await;
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

    /// Deliver a forwarded message to local subscribers
    async fn deliver_forward_message(&self, forward: rex_cluster::types::ForwardMessage) {
        // Save fields needed for ACK before moving
        let forward_id = forward.forward_id;
        let original_source = forward.original_source;
        let require_ack = forward.require_ack;
        let title = forward.title.clone();
        let payload = forward.payload;
        let is_broadcast = forward.is_broadcast;
        let is_group = forward.is_group;

        // Get system reference
        let system = {
            let system_lock = self.system.read();
            match system_lock.as_ref() {
                Some(s) => Arc::clone(s),
                None => {
                    tracing::warn!("No system reference available for message delivery");
                    return;
                }
            }
        };

        // Build RexData from payload
        // The payload is already the raw RexData bytes, unpack it
        let rex_data = RexData::unpack(bytes::BytesMut::from(payload.as_slice()));

        // Handle broadcast or group messages
        if is_broadcast || is_group {
            // Find all subscribers for this title
            let clients = system.find_all_by_title(&title, None);
            if clients.is_empty() {
                tracing::warn!(
                    "No local subscribers found for broadcast/group title: {}",
                    title
                );
            } else {
                tracing::info!(
                    "Delivering {} message to {} local subscribers",
                    if is_broadcast { "broadcast" } else { "group" },
                    clients.len()
                );
                for client in clients {
                    let client_id = client.id();
                    if let Err(e) = client.send_buf(rex_data.pack_ref()).await {
                        tracing::warn!("Failed to send to client {:032x}: {}", client_id, e);
                    }
                }
            }
            return;
        }

        // Handle unicast message
        let target_client = if forward.target_client_id != 0 {
            // Specific target client
            system.find_some_by_id(forward.target_client_id)
        } else {
            // Find any subscriber for this title
            system.find_one_by_title(&title, None)
        };

        match target_client {
            Some(client) => {
                let client_id = client.id();
                tracing::info!(
                    "Delivering forwarded message to local client {:032x}",
                    client_id
                );

                // Send the message
                if let Err(e) = client.send_buf(rex_data.pack_ref()).await {
                    tracing::warn!("Failed to send to client {:032x}: {}", client_id, e);
                    // Send failure ACK if requested
                    if require_ack {
                        self.send_forward_ack(
                            forward_id,
                            original_source,
                            false,
                            Some(e.to_string()),
                        )
                        .await;
                    }
                } else {
                    tracing::debug!(
                        "Successfully delivered forwarded message to {:032x}",
                        client_id
                    );
                    // Send success ACK if requested
                    if require_ack {
                        self.send_forward_ack(forward_id, original_source, true, None)
                            .await;
                    }
                }
            }
            None => {
                tracing::warn!("No local subscriber found for title: {}", title);
                // Send failure ACK if requested
                if require_ack {
                    self.send_forward_ack(
                        forward_id,
                        original_source,
                        false,
                        Some("No local subscriber".to_string()),
                    )
                    .await;
                }
            }
        }
    }

    /// Send forward acknowledgment back to source node
    async fn send_forward_ack(
        &self,
        forward_id: u64,
        original_source: u128,
        success: bool,
        error: Option<String>,
    ) {
        let local_node_id = self.local_node_id.to_string();
        let ack = rex_cluster::types::ForwardAckMessage {
            forward_id,
            from_node_id: local_node_id,
            original_source,
            success,
            error,
        };
        let msg = ClusterMessage::ForwardAck(ack);

        // Get transport to find the source node
        // For now, we just broadcast - the original node will recognize the forward_id
        // A more optimized approach would be to track pending forwards
        self.broadcast(msg).await;
    }
}
