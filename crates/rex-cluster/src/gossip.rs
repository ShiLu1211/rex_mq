//! Gossip Protocol Implementation
//!
//! Implements SWIM-like gossip protocol for node discovery and state synchronization

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tokio::time::interval;
use tracing::{debug, warn};

use crate::transport::ClusterTransport;
use crate::types::{ClusterMessage, NodeId, NodeInfo, NodeState, PingMessage, PongMessage};

/// Gossip protocol configuration
#[derive(Debug, Clone)]
pub struct GossipConfig {
    /// Gossip interval (ms)
    pub gossip_interval_ms: u64,
    /// Number of nodes to gossip with per round
    pub fanout: usize,
    /// Probe timeout (ms)
    pub probe_timeout_ms: u64,
    /// Max number of suspects before declaring node dead
    pub max_suspects: usize,
}

impl Default for GossipConfig {
    fn default() -> Self {
        Self {
            gossip_interval_ms: 1000,
            fanout: 3,
            probe_timeout_ms: 500,
            max_suspects: 3,
        }
    }
}

/// Gossip state for a node
#[derive(Debug, Clone)]
pub struct GossipState {
    /// Node information
    pub node_info: NodeInfo,
    /// Incarnation number (for handling conflicts)
    pub incarnation: u64,
    /// State vector version
    pub version: u64,
}

/// Gossip protocol handler
pub struct GossipProtocol {
    /// Local node ID
    local_node_id: NodeId,
    /// Transport layer
    transport: Arc<ClusterTransport>,
    /// Known nodes and their states
    members: DashMap<String, GossipState>,
    /// Configuration
    config: GossipConfig,
    /// Message channel
    message_tx: mpsc::UnboundedSender<ClusterMessage>,
    /// Shutdown flag
    shutdown: Arc<RwLock<bool>>,
}

impl GossipProtocol {
    /// Create a new gossip protocol handler
    pub fn new(
        local_node_id: NodeId,
        transport: Arc<ClusterTransport>,
        config: GossipConfig,
        message_tx: mpsc::UnboundedSender<ClusterMessage>,
    ) -> Self {
        Self {
            local_node_id,
            transport,
            members: DashMap::new(),
            config,
            message_tx,
            shutdown: Arc::new(RwLock::new(false)),
        }
    }

    /// Add a known node
    pub fn add_member(&self, node_info: NodeInfo) {
        let state = GossipState {
            node_info: node_info.clone(),
            incarnation: 0,
            version: 0,
        };
        self.members
            .insert(node_info.node_id.as_str().to_string(), state);
        debug!("Added member: {}", node_info.node_id.as_str());
    }

    /// Remove a node
    pub fn remove_member(&self, node_id: &str) {
        self.members.remove(node_id);
        debug!("Removed member: {}", node_id);
    }

    /// Start the gossip protocol
    pub fn start(&self) {
        let local_id = self.local_node_id.clone();
        let members = Arc::new(self.members.clone());
        let transport = self.transport.clone();
        let config = self.config.clone();
        let _message_tx = self.message_tx.clone();
        let shutdown = self.shutdown.clone();

        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_millis(config.gossip_interval_ms));

            loop {
                ticker.tick().await;

                if *shutdown.read() {
                    break;
                }

                // Select random nodes to gossip with
                let nodes: Vec<String> = members
                    .iter()
                    .filter(|entry| {
                        let id = entry.key();
                        let state = entry.value();
                        id != local_id.as_str() && state.node_info.state == NodeState::Active
                    })
                    .map(|entry| entry.key().clone())
                    .collect();

                if nodes.is_empty() {
                    continue;
                }

                // Gossip with fanout nodes
                let gossip_count = nodes.len().min(config.fanout);
                let selected: Vec<String> = nodes.into_iter().take(gossip_count).collect();

                for node_id in selected {
                    // Send ping with local node info
                    let ping = PingMessage {
                        node_id: local_id.to_string(),
                        timestamp: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_millis() as u64)
                            .unwrap_or(0),
                    };
                    if let Err(e) = transport
                        .send_to(&node_id, &ClusterMessage::Ping(ping))
                        .await
                    {
                        debug!("Failed to send ping to {}: {}", node_id, e);
                    }
                }
            }
        });
    }

    /// Stop the gossip protocol
    pub fn stop(&self) {
        *self.shutdown.write() = true;
    }

    /// Get all active members
    pub fn get_active_members(&self) -> Vec<NodeInfo> {
        self.members
            .iter()
            .filter(|entry| entry.value().node_info.state == NodeState::Active)
            .map(|entry| entry.value().node_info.clone())
            .collect()
    }

    /// Handle a ping message
    pub async fn handle_ping(&self, from_node_id: &NodeId, _ping: PingMessage) -> Result<()> {
        // Update member state
        if let Some(mut state) = self.members.get_mut(from_node_id.as_str()) {
            state.node_info.last_heartbeat = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            state.node_info.state = NodeState::Active;
        }

        // Send pong response
        let pong = PongMessage {
            node_id: self.local_node_id.to_string(),
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0),
        };
        let msg = ClusterMessage::Pong(pong);
        self.transport.send_to(from_node_id.as_str(), &msg).await?;

        Ok(())
    }

    /// Handle a pong message
    pub fn handle_pong(&self, from_node_id: &NodeId, _pong: PongMessage) {
        if let Some(mut state) = self.members.get_mut(from_node_id.as_str()) {
            state.node_info.last_heartbeat = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            state.node_info.state = NodeState::Active;
        }
    }

    /// Check for suspected/failed nodes
    pub fn check_suspects(&self) -> Vec<String> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let timeout = self.config.probe_timeout_ms * self.config.max_suspects as u64;
        let mut failed = Vec::new();

        self.members.retain(|id, state| {
            if id == self.local_node_id.as_str() {
                return true;
            }

            let elapsed = now.saturating_sub(state.node_info.last_heartbeat);

            if elapsed > timeout && state.node_info.state != NodeState::Left {
                // Node is suspected to be dead
                warn!(
                    "Node {} is suspected to be failed (elapsed: {}ms)",
                    id, elapsed
                );
                state.node_info.state = NodeState::Suspected;
                failed.push(id.clone());
                false // Remove from active members
            } else {
                true
            }
        });

        failed
    }

    /// Get member count
    pub fn member_count(&self) -> usize {
        self.members.len()
    }

    /// Export all members for sync
    pub fn export_members(&self) -> Vec<GossipState> {
        self.members.iter().map(|e| e.value().clone()).collect()
    }

    /// Import members from sync
    pub fn import_members(&self, states: Vec<GossipState>) {
        for state in states {
            self.members
                .insert(state.node_info.node_id.as_str().to_string(), state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gossip_config_default() {
        let config = GossipConfig::default();
        assert_eq!(config.gossip_interval_ms, 1000);
        assert_eq!(config.fanout, 3);
    }
}
