//! Cluster Manager
//!
//! Integrates all cluster components: NodeManager, FailoverManager, StateSyncer

use std::sync::Arc;

use anyhow::Result;
use tokio::sync::{broadcast, mpsc};
use tracing::info;

use crate::failover::{FailoverConfig, FailoverManager, NodeStatus};
use crate::route_table::GlobalRouteTable;
use crate::sync::StateSyncer;
use crate::transport::ClusterTransport;
use crate::types::{ClusterConfig, ClusterMessage, ClusterRole, NodeId, NodeInfo};

/// Integrated cluster manager
pub struct ClusterManager {
    /// Local node ID
    node_id: NodeId,
    /// Transport layer
    transport: Arc<ClusterTransport>,
    /// Route table
    route_table: Arc<GlobalRouteTable>,
    /// Failover manager
    failover: FailoverManager,
    /// State synchronizer
    state_syncer: Option<StateSyncer>,
    /// Current role
    role: parking_lot::RwLock<ClusterRole>,
    /// Known nodes
    nodes: dashmap::DashMap<String, NodeInfo>,
    /// Message sender to node manager
    message_tx: mpsc::UnboundedSender<ClusterMessage>,
    /// Shutdown signal
    shutdown_tx: broadcast::Sender<()>,
}

/// Configuration for ClusterManager
#[derive(Debug, Clone)]
pub struct ClusterManagerConfig {
    /// Cluster configuration
    pub cluster: ClusterConfig,
    /// Enable state sync
    pub enable_state_sync: bool,
    /// Failover configuration
    pub failover_config: FailoverConfig,
}

impl Default for ClusterManagerConfig {
    fn default() -> Self {
        Self {
            cluster: ClusterConfig::default(),
            enable_state_sync: true,
            failover_config: FailoverConfig::default(),
        }
    }
}

impl ClusterManager {
    /// Create a new cluster manager
    pub fn new(
        config: ClusterManagerConfig,
    ) -> Result<(Self, mpsc::UnboundedReceiver<ClusterMessage>)> {
        let (message_tx, message_rx) = mpsc::unbounded_channel();
        let (shutdown_tx, _) = broadcast::channel(1);

        let node_id = config.cluster.node_id.clone();
        let route_table = Arc::new(GlobalRouteTable::with_local_node(node_id.clone()));

        // Create transport
        let (transport_tx, _transport_rx) = mpsc::unbounded_channel();
        let transport = Arc::new(ClusterTransport::new(node_id.clone(), transport_tx));

        // Create failover manager
        let failover = FailoverManager::with_config(
            node_id.clone(),
            transport.clone(),
            route_table.clone(),
            config.failover_config,
        );

        // Create state syncer if enabled
        let state_syncer = if config.enable_state_sync {
            Some(StateSyncer::new(
                node_id.clone(),
                transport.clone(),
                route_table.clone(),
            ))
        } else {
            None
        };

        let manager = Self {
            node_id,
            transport,
            route_table,
            failover,
            state_syncer,
            role: parking_lot::RwLock::new(ClusterRole::Standalone),
            nodes: dashmap::DashMap::new(),
            message_tx,
            shutdown_tx,
        };

        Ok((manager, message_rx))
    }

    /// Start the cluster manager
    pub fn start(&self) {
        info!("Starting cluster manager for node {}", self.node_id);

        // Start failover manager
        self.failover.start();

        info!("Cluster manager started for node {}", self.node_id);
    }

    /// Handle node join
    pub fn handle_node_join(&self, node_info: NodeInfo) {
        let node_id = node_info.node_id.to_string();

        // Add to route table
        self.route_table
            .add_node(node_id.clone(), node_info.listen_addr.to_string());

        // Add to nodes map
        self.nodes.insert(node_id.clone(), node_info);

        // Record in failover manager
        self.failover.add_node(node_id.clone());

        info!("Node {} joined the cluster", node_id);
    }

    /// Record heartbeat from leader
    pub fn record_leader_heartbeat(&self, leader_id: &str) {
        // Record heartbeat for failover
        self.failover.record_heartbeat(leader_id);
    }

    /// Get local node ID
    pub fn node_id(&self) -> &NodeId {
        &self.node_id
    }

    /// Get current role
    pub fn role(&self) -> ClusterRole {
        *self.role.read()
    }

    /// Check if this node is leader
    pub fn is_leader(&self) -> bool {
        *self.role.read() == ClusterRole::Leader
    }

    /// Set role
    pub fn set_role(&self, role: ClusterRole) {
        *self.role.write() = role;
    }

    /// Get all known nodes
    pub fn nodes(&self) -> Vec<NodeInfo> {
        self.nodes.iter().map(|e| e.value().clone()).collect()
    }

    /// Get route table
    pub fn route_table(&self) -> &Arc<GlobalRouteTable> {
        &self.route_table
    }

    /// Get node status
    pub fn get_node_status(&self, node_id: &str) -> Option<NodeStatus> {
        self.failover.get_node_status(node_id)
    }

    /// Get all node statuses
    pub fn get_all_node_statuses(&self) -> Vec<(String, NodeStatus)> {
        self.failover.get_all_node_status()
    }

    /// Get state syncer
    pub fn state_syncer(&self) -> Option<&StateSyncer> {
        self.state_syncer.as_ref()
    }

    /// Shutdown the cluster manager
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
        info!("Cluster manager shutdown");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cluster_manager_config_default() {
        let config = ClusterManagerConfig::default();
        assert!(config.enable_state_sync);
    }
}
