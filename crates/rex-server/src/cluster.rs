//! Cluster Integration
//!
//! Provides integration between RexSystem and the cluster module

pub mod server_cluster;

use std::sync::Arc;

use rex_cluster::route_table::GlobalRouteTable;
use rex_cluster::types::NodeId;
use tokio::sync::mpsc;

/// Cluster integration for RexSystem
pub struct ClusterIntegration {
    /// Route table for cluster
    pub route_table: Arc<GlobalRouteTable>,
    /// Local node ID
    pub local_node_id: NodeId,
    /// Message sender for forwarding to other nodes
    forward_tx: Option<mpsc::UnboundedSender<ForwardRequest>>,
}

/// Request to forward a message to another node
#[derive(Debug, Clone)]
pub struct ForwardRequest {
    /// Original source client ID
    pub source_client_id: u128,
    /// Target client ID (0 if not specific)
    pub target_client_id: u128,
    /// Message title
    pub title: String,
    /// Message payload
    pub payload: Vec<u8>,
    /// Message type
    pub msg_type: ForwardType,
}

/// Type of forward
#[derive(Debug, Clone, Copy)]
pub enum ForwardType {
    /// Unicast (single target)
    Unicast,
    /// Multicast (one of group)
    Group,
    /// Broadcast (all subscribers)
    Broadcast,
}

impl ClusterIntegration {
    /// Create a new cluster integration
    pub fn new(node_id: String) -> Self {
        let local_node_id = NodeId::new(node_id);
        let route_table = Arc::new(GlobalRouteTable::with_local_node(local_node_id.clone()));

        Self {
            route_table,
            local_node_id,
            forward_tx: None,
        }
    }

    /// Set forward message sender
    pub fn set_forward_sender(&mut self, tx: mpsc::UnboundedSender<ForwardRequest>) {
        self.forward_tx = Some(tx);
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

    /// Register a title subscription
    pub fn register_title(&self, _title: &str) {
        // Title routing is handled via consistent hash
        // No need to explicitly register
    }

    /// Get node for a client
    pub fn get_node_for_client(&self, client_id: &u128) -> Option<String> {
        self.route_table.get_node_by_client(client_id)
    }

    /// Get node for a title
    pub fn get_node_for_title(&self, title: &str) -> Option<String> {
        self.route_table.get_node_by_title(title)
    }

    /// Check if a client is local to this node
    pub fn is_client_local(&self, client_id: &u128) -> bool {
        self.route_table.is_client_local(client_id)
    }

    /// Forward a message to another node
    pub async fn forward_message(&self, request: ForwardRequest) -> bool {
        if let Some(tx) = &self.forward_tx {
            return tx.send(request).is_ok();
        }
        false
    }

    /// Get all known nodes
    pub fn get_nodes(&self) -> Vec<String> {
        self.route_table.get_all_nodes()
    }

    /// Add a node to the cluster
    pub fn add_node(&self, node_id: String, addr: String) {
        self.route_table.add_node(node_id, addr);
    }

    /// Remove a node from the cluster
    pub fn remove_node(&self, node_id: &str) {
        self.route_table.remove_node(node_id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_integration_creation() {
        let cluster = ClusterIntegration::new("test-node".to_string());
        assert_eq!(cluster.local_node_id.as_str(), "test-node");
    }

    #[test]
    fn test_register_client() {
        let cluster = ClusterIntegration::new("test-node".to_string());
        cluster.register_client(12345);
        assert!(cluster.is_client_local(&12345));
    }

    #[test]
    fn test_unregister_client() {
        let cluster = ClusterIntegration::new("test-node".to_string());
        cluster.register_client(12345);
        cluster.unregister_client(&12345);
        assert!(!cluster.is_client_local(&12345));
    }
}
