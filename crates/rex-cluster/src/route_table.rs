//! Global Route Table
//!
//! Manages routing information for the entire cluster

use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tracing::debug;

use crate::hash_ring::HashRing;
use crate::types::NodeId;

/// Global route table for the cluster
#[derive(Debug)]
pub struct GlobalRouteTable {
    /// Title to nodes mapping (consistent hash ring)
    title_to_nodes: RwLock<HashRing>,
    /// Client ID to node mapping
    client_id_to_node: DashMap<u128, String>,
    /// Node ID to address mapping
    node_id_to_addr: DashMap<String, String>,
    /// Local node ID
    local_node_id: RwLock<Option<NodeId>>,
    /// Route table version
    version: RwLock<u64>,
}

impl GlobalRouteTable {
    /// Create a new global route table
    pub fn new() -> Self {
        Self {
            title_to_nodes: RwLock::new(HashRing::new()),
            client_id_to_node: DashMap::new(),
            node_id_to_addr: DashMap::new(),
            local_node_id: RwLock::new(None),
            version: RwLock::new(0),
        }
    }

    /// Create with local node ID
    pub fn with_local_node(node_id: NodeId) -> Self {
        let table = Self::new();
        *table.local_node_id.write() = Some(node_id);
        table
    }

    /// Set local node ID
    pub fn set_local_node(&self, node_id: NodeId) {
        *self.local_node_id.write() = Some(node_id);
    }

    /// Add a node to the route table
    pub fn add_node(&self, node_id: String, addr: String) {
        self.node_id_to_addr.insert(node_id.clone(), addr);

        let mut ring = self.title_to_nodes.write();
        ring.add_node(node_id);

        *self.version.write() += 1;
        debug!(
            "Added node to route table, version: {}",
            *self.version.read()
        );
    }

    /// Remove a node from the route table
    pub fn remove_node(&self, node_id: &str) {
        self.node_id_to_addr.remove(node_id);

        let mut ring = self.title_to_nodes.write();
        ring.remove_node(node_id);

        *self.version.write() += 1;
        debug!(
            "Removed node from route table, version: {}",
            *self.version.read()
        );
    }

    /// Register a client to this node
    pub fn register_client(&self, client_id: u128, node_id: &str) {
        self.client_id_to_node
            .insert(client_id, node_id.to_string());
        *self.version.write() += 1;
    }

    /// Unregister a client
    pub fn unregister_client(&self, client_id: &u128) {
        self.client_id_to_node.remove(client_id);
        *self.version.write() += 1;
    }

    /// Get the node for a title
    pub fn get_node_by_title(&self, title: &str) -> Option<String> {
        let ring = self.title_to_nodes.read();
        ring.get(title).map(|s| s.to_string())
    }

    /// Get the node for a client ID
    pub fn get_node_by_client(&self, client_id: &u128) -> Option<String> {
        self.client_id_to_node
            .get(client_id)
            .map(|r| r.value().clone())
    }

    /// Get node address by node ID
    pub fn get_node_addr(&self, node_id: &str) -> Option<String> {
        self.node_id_to_addr.get(node_id).map(|r| r.value().clone())
    }

    /// Get all known nodes
    pub fn get_all_nodes(&self) -> Vec<String> {
        self.node_id_to_addr
            .iter()
            .map(|r| r.key().clone())
            .collect()
    }

    /// Get N nodes for a title (for replication)
    pub fn get_nodes_by_title(&self, title: &str, n: usize) -> Vec<String> {
        let ring = self.title_to_nodes.read();
        ring.get_n(title, n)
            .into_iter()
            .map(|s| s.to_string())
            .collect()
    }

    /// Check if a client is local to this node
    pub fn is_client_local(&self, client_id: &u128) -> bool {
        let local_id = self.local_node_id.read();
        if let Some(local) = local_id.as_ref()
            && let Some(node) = self.get_node_by_client(client_id)
        {
            return node == local.as_str();
        }
        false
    }

    /// Get current version
    pub fn version(&self) -> u64 {
        *self.version.read()
    }

    /// Get local node ID
    pub fn local_node(&self) -> Option<NodeId> {
        self.local_node_id.read().clone()
    }

    /// Serialize for network sync
    pub fn export_state(&self) -> RouteTableState {
        let _ring = self.title_to_nodes.read();
        let client_id_to_node: Vec<(u128, String)> = self
            .client_id_to_node
            .iter()
            .map(|r| (*r.key(), r.value().clone()))
            .collect();
        let node_id_to_addr: Vec<(String, String)> = self
            .node_id_to_addr
            .iter()
            .map(|r| (r.key().clone(), r.value().clone()))
            .collect();

        RouteTableState {
            version: *self.version.read(),
            client_id_to_node,
            node_id_to_addr,
        }
    }

    /// Import state from network
    pub fn import_state(&self, state: RouteTableState) {
        {
            let mut ring = self.title_to_nodes.write();
            *ring = HashRing::new();
            for (node_id, _) in &state.node_id_to_addr {
                ring.add_node(node_id.clone());
            }
        }

        {
            self.client_id_to_node.clear();
            for (client_id, node_id) in &state.client_id_to_node {
                self.client_id_to_node.insert(*client_id, node_id.clone());
            }
        }

        {
            self.node_id_to_addr.clear();
            for (node_id, addr) in &state.node_id_to_addr {
                self.node_id_to_addr.insert(node_id.clone(), addr.clone());
            }
        }

        *self.version.write() = state.version;
    }
}

impl Default for GlobalRouteTable {
    fn default() -> Self {
        Self::new()
    }
}

/// Route table state for synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteTableState {
    /// Version number
    pub version: u64,
    /// Client ID to node mapping
    pub client_id_to_node: Vec<(u128, String)>,
    /// Node ID to address mapping
    pub node_id_to_addr: Vec<(String, String)>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_route_table_empty() {
        let table = GlobalRouteTable::new();
        assert!(table.get_node_by_title("test").is_none());
    }

    #[test]
    fn test_route_table_add_node() {
        let table = GlobalRouteTable::new();
        table.add_node("node1".to_string(), "127.0.0.1:9001".to_string());

        assert_eq!(table.get_all_nodes(), vec!["node1"]);
    }

    #[test]
    fn test_route_table_client_registration() {
        let table = GlobalRouteTable::new();
        table.add_node("node1".to_string(), "127.0.0.1:9001".to_string());
        table.register_client(12345, "node1");

        assert_eq!(table.get_node_by_client(&12345), Some("node1".to_string()));
    }

    #[test]
    fn test_route_table_title_routing() {
        let table = GlobalRouteTable::new();
        table.add_node("node1".to_string(), "127.0.0.1:9001".to_string());
        table.add_node("node2".to_string(), "127.0.0.1:9002".to_string());

        let node = table.get_node_by_title("test-title");
        assert!(node.is_some());
    }

    #[test]
    fn test_route_table_export_import() {
        let table = GlobalRouteTable::new();
        table.add_node("node1".to_string(), "127.0.0.1:9001".to_string());
        table.register_client(12345, "node1");

        let state = table.export_state();
        assert_eq!(state.version, 2);
        assert_eq!(state.client_id_to_node.len(), 1);

        let table2 = GlobalRouteTable::new();
        table2.import_state(state);

        assert_eq!(table2.get_all_nodes(), vec!["node1"]);
        assert_eq!(table2.get_node_by_client(&12345), Some("node1".to_string()));
    }
    // ---------- New tests (PR 1: rex-cluster test coverage) ----------

    #[test]
    fn register_client_then_get_node_by_client_returns_node() {
        let table = GlobalRouteTable::new();
        table.add_node("node-1".to_string(), "127.0.0.1:9001".to_string());
        table.register_client(0xCAFEu128, "node-1");

        assert_eq!(
            table.get_node_by_client(&0xCAFEu128),
            Some("node-1".to_string())
        );
    }

    #[test]
    fn register_client_then_unregister_client_returns_none() {
        let table = GlobalRouteTable::new();
        table.add_node("node-1".to_string(), "127.0.0.1:9001".to_string());
        table.register_client(0xCAFEu128, "node-1");
        table.unregister_client(&0xCAFEu128);

        assert!(table.get_node_by_client(&0xCAFEu128).is_none());
    }

    #[test]
    fn is_client_local_for_unknown_client_returns_false() {
        let table = GlobalRouteTable::with_local_node(NodeId::new("local-node"));
        assert!(!table.is_client_local(&0xCAFEu128));
    }

    #[test]
    fn is_client_local_returns_true_only_for_local_node_mapping() {
        let table = GlobalRouteTable::with_local_node(NodeId::new("local-node"));
        table.add_node("local-node".to_string(), "127.0.0.1:9001".to_string());
        table.add_node("remote-node".to_string(), "127.0.0.1:9002".to_string());

        table.register_client(0xCAFEu128, "local-node");
        table.register_client(0xBABEu128, "remote-node");

        assert!(table.is_client_local(&0xCAFEu128));
        assert!(!table.is_client_local(&0xBABEu128));
    }

    #[test]
    fn add_node_then_remove_node_clears_addr_and_ring_entry() {
        let table = GlobalRouteTable::new();
        table.add_node("node-1".to_string(), "127.0.0.1:9001".to_string());
        assert_eq!(table.get_node_addr("node-1"), Some("127.0.0.1:9001".into()));

        table.remove_node("node-1");
        assert!(table.get_node_addr("node-1").is_none());
        assert!(!table.get_all_nodes().contains(&"node-1".to_string()));
    }

    #[test]
    fn get_node_addr_for_unknown_node_returns_none() {
        let table = GlobalRouteTable::new();
        assert!(table.get_node_addr("never-added").is_none());
    }

    #[test]
    fn get_nodes_by_title_returns_at_most_n_distinct_nodes() {
        let table = GlobalRouteTable::new();
        table.add_node("node-1".to_string(), "127.0.0.1:9001".to_string());
        table.add_node("node-2".to_string(), "127.0.0.1:9002".to_string());
        table.add_node("node-3".to_string(), "127.0.0.1:9003".to_string());

        // Ask for 5; only 3 nodes are registered.
        let nodes = table.get_nodes_by_title("any-title", 5);
        assert_eq!(nodes.len(), 3);
        // All returned nodes are unique.
        let unique: std::collections::HashSet<_> = nodes.iter().collect();
        assert_eq!(unique.len(), 3);
    }

    #[test]
    fn set_local_node_replaces_previous_local() {
        let table = GlobalRouteTable::with_local_node(NodeId::new("first"));
        assert_eq!(table.local_node().unwrap().as_str(), "first");

        table.set_local_node(NodeId::new("second"));
        assert_eq!(table.local_node().unwrap().as_str(), "second");
    }

    #[test]
    fn version_increments_on_each_modification() {
        let table = GlobalRouteTable::new();
        let v0 = table.version();

        table.add_node("node-1".to_string(), "127.0.0.1:9001".to_string());
        let v1 = table.version();
        assert_eq!(v1, v0 + 1);

        table.add_node("node-2".to_string(), "127.0.0.1:9002".to_string());
        let v2 = table.version();
        assert_eq!(v2, v1 + 1);

        table.register_client(0xCAFEu128, "node-1");
        assert_eq!(table.version(), v2 + 1);

        table.unregister_client(&0xCAFEu128);
        assert_eq!(table.version(), v2 + 2);

        table.remove_node("node-1");
        assert_eq!(table.version(), v2 + 3);
    }
}
