//! RexMQ Cluster Types
//!
//! Core data structures for cluster communication. Only the variants
//! and fields that are actually wired into the live cluster path
//! (`NodeManager`, `Forwarder`, `ServerClusterManager`) remain; Raft-era
//! state machines (leader election, log replication, state sync) were
//! speculative scaffolding that never reached a server.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

/// Unique identifier for a cluster node
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub String);

impl NodeId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for NodeId {
    fn from(s: String) -> Self {
        NodeId(s)
    }
}

/// Information about a cluster node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier
    pub node_id: NodeId,
    /// Socket address for cluster communication
    pub listen_addr: SocketAddr,
    /// Last heartbeat timestamp (ms since UNIX epoch)
    pub last_heartbeat: u64,
}

impl NodeInfo {
    pub fn new(node_id: NodeId, listen_addr: SocketAddr) -> Self {
        Self {
            node_id,
            listen_addr,
            last_heartbeat: 0,
        }
    }
}

/// Configuration for cluster mode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Whether cluster mode is enabled
    pub enabled: bool,
    /// This node's ID
    pub node_id: NodeId,
    /// Socket address for cluster internal communication
    pub listen_addr: SocketAddr,
    /// Seed nodes for initial cluster discovery
    pub seed_nodes: Vec<SocketAddr>,
    /// Cluster communication timeout
    pub communication_timeout_ms: u64,
    /// Heartbeat interval (ms)
    pub heartbeat_interval_ms: u64,
    /// Maximum retries for failed messages
    pub max_retries: u32,
}

impl Default for ClusterConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            node_id: NodeId::new(rand::random::<u128>().to_string()),
            listen_addr: "127.0.0.1:0".parse().unwrap_or_else(|_| {
                std::net::SocketAddr::new(std::net::IpAddr::from([127, 0, 0, 1]), 0)
            }),
            seed_nodes: Vec::new(),
            communication_timeout_ms: 1000,
            heartbeat_interval_ms: 1000,
            max_retries: 3,
        }
    }
}

impl ClusterConfig {
    pub fn new(node_id: NodeId, listen_addr: SocketAddr) -> Self {
        Self {
            enabled: true,
            node_id,
            listen_addr,
            ..Default::default()
        }
    }

    pub fn with_seed_nodes(mut self, seeds: Vec<SocketAddr>) -> Self {
        self.seed_nodes = seeds;
        self
    }

    pub fn with_heartbeat_interval(mut self, ms: u64) -> Self {
        self.heartbeat_interval_ms = ms;
        self
    }
}

/// Wire-level messages exchanged between cluster nodes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterMessage {
    /// Join request from a new node
    Join(NodeInfo),
    /// Response to Join with known nodes list
    NodeList(Vec<NodeInfo>),
    /// Heartbeat message (periodic liveness)
    Heartbeat(HeartbeatMessage),
    /// Forward message to another node (cross-node delivery)
    Forward(ForwardMessage),
    /// Forward acknowledgment
    ForwardAck(ForwardAckMessage),
    /// Ping for health check
    Ping(PingMessage),
    /// Title registration propagated to other nodes
    TitleRegister(TitleRegisterMessage),
    /// Title unregistration propagated to other nodes
    TitleUnregister(TitleUnregisterMessage),
}

/// Heartbeat message — periodic liveness signal from a node.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatMessage {
    /// Originating node ID (effectively the "leader" since we don't run
    /// leader election; this is the local node id of the broadcaster).
    pub leader_id: NodeId,
}

/// Ping message for health check.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingMessage {
    /// Node ID sending the ping
    pub node_id: String,
    /// Timestamp for RTT calculation
    pub timestamp: u64,
}

/// Forward message for cross-node message delivery.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardMessage {
    pub forward_id: u64,
    pub original_source: u128,
    pub target_client_id: u128,
    pub title: String,
    pub payload: Vec<u8>,
    pub is_group: bool,
    pub is_broadcast: bool,
    pub require_ack: bool,
}

/// Forward acknowledgment message.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForwardAckMessage {
    /// Original forward ID
    pub forward_id: u64,
    /// Source node that received the message
    pub from_node_id: String,
    /// Original source client ID
    pub original_source: u128,
    /// Whether delivery was successful
    pub success: bool,
    /// Error message if failed
    pub error: Option<String>,
}

/// Title registration message for cluster propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleRegisterMessage {
    /// Node that registered the title
    pub node_id: String,
    /// Registered title
    pub title: String,
    /// Client ID that registered (optional, 0 if broadcast)
    pub client_id: u128,
}

/// Title unregistration message for cluster propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleUnregisterMessage {
    /// Node that unregistered the title
    pub node_id: String,
    /// Unregistered title
    pub title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_node_id() {
        let id = NodeId::new("node-1");
        assert_eq!(id.as_str(), "node-1");
    }

    #[test]
    fn test_cluster_config() {
        let addr: std::net::SocketAddr = "127.0.0.1:9001".parse().unwrap_or_else(|_| {
            std::net::SocketAddr::new(std::net::IpAddr::from([127, 0, 0, 1]), 9001)
        });
        let seed: std::net::SocketAddr = "127.0.0.1:9000".parse().unwrap_or_else(|_| {
            std::net::SocketAddr::new(std::net::IpAddr::from([127, 0, 0, 1]), 9000)
        });

        let config = ClusterConfig::new(NodeId::new("node-1"), addr).with_seed_nodes(vec![seed]);

        assert!(config.enabled);
        assert_eq!(config.seed_nodes.len(), 1);
    }
}
