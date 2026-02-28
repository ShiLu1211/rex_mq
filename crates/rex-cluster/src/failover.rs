//! Failover Manager
//!
//! Handles failure detection and failover for cluster nodes

use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, info, warn};

use crate::route_table::GlobalRouteTable;
use crate::transport::ClusterTransport;
use crate::types::NodeId;

/// Failure detection status
#[derive(Debug, Clone, PartialEq)]
pub enum NodeStatus {
    /// Node is alive and responsive
    Alive,
    /// Node is suspected to be down
    Suspected,
    /// Node is confirmed down
    Dead,
}

/// Information about a cluster node's health
#[derive(Debug, Clone)]
pub struct NodeHealth {
    /// Node ID
    pub node_id: String,
    /// Current status
    pub status: NodeStatus,
    /// Last time we received a heartbeat from this node
    pub last_heartbeat: u64,
    /// Number of consecutive failures
    pub failure_count: u32,
    /// Last time we checked this node
    pub last_check: u64,
}

/// Failover manager for handling node failures
#[allow(dead_code)]
pub struct FailoverManager {
    /// Local node ID
    local_node_id: NodeId,
    /// Transport layer for sending messages
    transport: Arc<ClusterTransport>,
    /// Route table for updating routing info
    route_table: Arc<GlobalRouteTable>,
    /// Node health information
    node_health: DashMap<String, NodeHealth>,
    /// Channel for failover events
    failover_tx: Option<mpsc::UnboundedSender<FailoverEvent>>,
    /// Shutdown signal
    shutdown_rx: Arc<RwLock<Option<mpsc::Receiver<()>>>>,
    /// Configuration
    config: FailoverConfig,
}

/// Configuration for failover
#[derive(Debug, Clone)]
pub struct FailoverConfig {
    /// How many consecutive failures before marking node as dead
    pub max_failures: u32,
    /// How often to check node health (ms)
    pub health_check_interval_ms: u64,
    /// How long without heartbeat before suspecting node (ms)
    pub suspect_timeout_ms: u64,
    /// How long without heartbeat before marking node as dead (ms)
    pub dead_timeout_ms: u64,
}

impl Default for FailoverConfig {
    fn default() -> Self {
        Self {
            max_failures: 3,
            health_check_interval_ms: 5000,
            suspect_timeout_ms: 15000,
            dead_timeout_ms: 30000,
        }
    }
}

/// Failover event types
#[derive(Debug, Clone)]
pub enum FailoverEvent {
    /// A node is suspected to be down
    NodeSuspected { node_id: String, failure_count: u32 },
    /// A node is confirmed dead
    NodeDead { node_id: String },
    /// A dead node has recovered
    NodeRecovered { node_id: String },
    /// We need to elect a new leader
    LeaderFailed { old_leader_id: String },
}

impl FailoverManager {
    /// Create a new failover manager
    pub fn new(
        local_node_id: NodeId,
        transport: Arc<ClusterTransport>,
        route_table: Arc<GlobalRouteTable>,
    ) -> Self {
        Self {
            local_node_id,
            transport,
            route_table,
            node_health: DashMap::new(),
            failover_tx: None,
            shutdown_rx: Arc::new(RwLock::new(None)),
            config: FailoverConfig::default(),
        }
    }

    /// Create with custom config
    pub fn with_config(
        local_node_id: NodeId,
        transport: Arc<ClusterTransport>,
        route_table: Arc<GlobalRouteTable>,
        config: FailoverConfig,
    ) -> Self {
        Self {
            local_node_id,
            transport,
            route_table,
            node_health: DashMap::new(),
            failover_tx: None,
            shutdown_rx: Arc::new(RwLock::new(None)),
            config,
        }
    }

    /// Set failover event sender
    pub fn set_event_sender(&mut self, tx: mpsc::UnboundedSender<FailoverEvent>) {
        self.failover_tx = Some(tx);
    }

    /// Start the failover manager background task
    pub fn start(&self) {
        let _transport = self.transport.clone();
        let route_table = self.route_table.clone();
        let node_health = self.node_health.clone();
        let config = self.config.clone();
        let failover_tx = self.failover_tx.clone();

        tokio::spawn(async move {
            let mut interval_timer =
                interval(Duration::from_millis(config.health_check_interval_ms));

            loop {
                interval_timer.tick().await;

                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);

                // Check all known nodes
                let nodes: Vec<String> = route_table.get_all_nodes();

                for node_id in nodes {
                    Self::check_node_health(&node_id, now, &node_health, &config, &failover_tx)
                        .await;
                }
            }
        });
    }

    /// Check health of a single node
    async fn check_node_health(
        node_id: &str,
        now: u64,
        node_health: &DashMap<String, NodeHealth>,
        config: &FailoverConfig,
        failover_tx: &Option<mpsc::UnboundedSender<FailoverEvent>>,
    ) {
        let mut should_remove = false;
        let _is_dead = false;
        let _node_id_str = node_id.to_string();

        if let Some(mut health) = node_health.get_mut(node_id) {
            let time_since_heartbeat = now.saturating_sub(health.last_heartbeat);

            if time_since_heartbeat > config.dead_timeout_ms {
                // Node is dead
                health.status = NodeStatus::Dead;
                should_remove = true;

                warn!("Node {} is dead", node_id);

                // Send failover event
                if let Some(tx) = failover_tx {
                    let _ = tx.send(FailoverEvent::NodeDead {
                        node_id: node_id.to_string(),
                    });
                }
            } else if time_since_heartbeat > config.suspect_timeout_ms {
                // Node is suspected
                if health.status != NodeStatus::Suspected {
                    health.status = NodeStatus::Suspected;
                    health.failure_count += 1;

                    debug!("Node {} is suspected to be down", node_id);

                    if let Some(tx) = failover_tx {
                        let _ = tx.send(FailoverEvent::NodeSuspected {
                            node_id: node_id.to_string(),
                            failure_count: health.failure_count,
                        });
                    }
                }
            }

            health.last_check = now;
        }

        if should_remove {
            // Remove dead node from route table
            // Note: In a real implementation, we'd want to be more careful here
            // to avoid removing nodes that are temporarily unreachable
        }
    }

    /// Record a heartbeat from a node
    pub fn record_heartbeat(&self, node_id: &str) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if let Some(mut health) = self.node_health.get_mut(node_id) {
            health.last_heartbeat = now;
            health.failure_count = 0;
            if health.status == NodeStatus::Suspected || health.status == NodeStatus::Dead {
                health.status = NodeStatus::Alive;
                info!("Node {} has recovered", node_id);

                if let Some(tx) = &self.failover_tx {
                    let _ = tx.send(FailoverEvent::NodeRecovered {
                        node_id: node_id.to_string(),
                    });
                }
            }
        } else {
            // New node, add to health tracking
            let health = NodeHealth {
                node_id: node_id.to_string(),
                status: NodeStatus::Alive,
                last_heartbeat: now,
                failure_count: 0,
                last_check: now,
            };
            self.node_health.insert(node_id.to_string(), health);
        }
    }

    /// Get status of a node
    pub fn get_node_status(&self, node_id: &str) -> Option<NodeStatus> {
        self.node_health.get(node_id).map(|h| h.status.clone())
    }

    /// Get all nodes with their status
    pub fn get_all_node_status(&self) -> Vec<(String, NodeStatus)> {
        self.node_health
            .iter()
            .map(|e| (e.key().clone(), e.value().status.clone()))
            .collect()
    }

    /// Check if a node is alive
    pub fn is_node_alive(&self, node_id: &str) -> bool {
        self.node_health
            .get(node_id)
            .map(|h| h.status == NodeStatus::Alive)
            .unwrap_or(false)
    }

    /// Add a node to tracking
    pub fn add_node(&self, node_id: String) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        if !self.node_health.contains_key(&node_id) {
            let health = NodeHealth {
                node_id: node_id.clone(),
                status: NodeStatus::Alive,
                last_heartbeat: now,
                failure_count: 0,
                last_check: now,
            };
            self.node_health.insert(node_id, health);
        }
    }

    /// Remove a node from tracking
    pub fn remove_node(&self, node_id: &str) {
        self.node_health.remove(node_id);
    }

    /// Handle leader failure
    pub fn handle_leader_failure(&self, leader_id: &str) {
        if let Some(tx) = &self.failover_tx {
            let _ = tx.send(FailoverEvent::LeaderFailed {
                old_leader_id: leader_id.to_string(),
            });
        }
    }

    /// Get the config
    pub fn config(&self) -> &FailoverConfig {
        &self.config
    }

    /// Update config
    pub fn set_config(&mut self, config: FailoverConfig) {
        self.config = config;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_failover_config_default() {
        let config = FailoverConfig::default();
        assert_eq!(config.max_failures, 3);
        assert_eq!(config.health_check_interval_ms, 5000);
    }

    #[test]
    fn test_node_status_comparison() {
        assert_eq!(NodeStatus::Alive, NodeStatus::Alive);
        assert_ne!(NodeStatus::Alive, NodeStatus::Dead);
    }
}
