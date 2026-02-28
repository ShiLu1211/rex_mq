//! RexMQ Cluster Types
//!
//! Core data structures for cluster communication and management

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

/// State of a cluster node
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum NodeState {
    /// Node is joining the cluster
    Joining,
    /// Node is active and healthy
    #[default]
    Active,
    /// Node is suspected to be failed
    Suspected,
    /// Node has left the cluster
    Left,
}

/// Information about a cluster node
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    /// Unique node identifier
    pub node_id: NodeId,
    /// Socket address for cluster communication
    pub listen_addr: SocketAddr,
    /// Whether this node is the leader
    pub is_leader: bool,
    /// Current state of the node
    pub state: NodeState,
    /// Last heartbeat timestamp (ms)
    pub last_heartbeat: u64,
    /// Current term (for Raft)
    pub term: u64,
    /// Node version/generation
    pub version: u64,
}

impl NodeInfo {
    pub fn new(node_id: NodeId, listen_addr: SocketAddr) -> Self {
        Self {
            node_id,
            listen_addr,
            is_leader: false,
            state: NodeState::Active,
            last_heartbeat: 0,
            term: 0,
            version: 0,
        }
    }

    pub fn with_leader(mut self, is_leader: bool) -> Self {
        self.is_leader = is_leader;
        self
    }
}

/// Role of the local node in the cluster
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ClusterRole {
    /// Follower node
    Follower,
    /// Candidate node (election in progress)
    Candidate,
    /// Leader node
    Leader,
    /// Not part of any cluster (standalone mode)
    #[default]
    Standalone,
}

/// Configuration for cluster mode
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
    /// Election timeout range (ms)
    pub election_timeout_min_ms: u64,
    pub election_timeout_max_ms: u64,
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
            election_timeout_min_ms: 5000,
            election_timeout_max_ms: 10000,
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
}

/// Message types for inter-node communication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClusterMessage {
    /// Join request from a new node
    Join(NodeInfo),
    /// Response to Join with known nodes list
    NodeList(Vec<NodeInfo>),
    /// Heartbeat message
    Heartbeat(HeartbeatMessage),
    /// Request for vote (election)
    RequestVote(RequestVoteMessage),
    /// Response to vote request
    VoteResponse(VoteResponseMessage),
    /// Append entries (log replication)
    AppendEntries(AppendEntriesMessage),
    /// Response to append entries
    AppendEntriesResponse(AppendEntriesResponse),
    /// Forward message to another node
    Forward(ForwardMessage),
    /// Forward acknowledgment
    ForwardAck(ForwardAckMessage),
    /// State sync request
    StateSyncRequest(StateSyncRequest),
    /// State sync response
    StateSyncResponse(StateSyncResponse),
    /// Ping for health check
    Ping(PingMessage),
    /// Pong response
    Pong(PongMessage),
    /// Title registration propagated to other nodes
    TitleRegister(TitleRegisterMessage),
    /// Title unregistration propagated to other nodes
    TitleUnregister(TitleUnregisterMessage),
}

/// Heartbeat message for leader to maintain authority
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HeartbeatMessage {
    pub term: u64,
    pub leader_id: NodeId,
    pub leader_commit: u64,
}

/// Ping message for health check
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PingMessage {
    /// Node ID sending the ping
    pub node_id: String,
    /// Timestamp for RTT calculation
    pub timestamp: u64,
}

/// Pong message for health check response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PongMessage {
    /// Node ID sending the pong
    pub node_id: String,
    /// Original timestamp from ping
    pub timestamp: u64,
}

/// Request vote message for election
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestVoteMessage {
    pub term: u64,
    pub candidate_id: NodeId,
    pub last_log_index: u64,
    pub last_log_term: u64,
}

/// Response to vote request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoteResponseMessage {
    pub term: u64,
    pub vote_granted: bool,
    pub voter_id: NodeId,
}

/// Append entries message for log replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesMessage {
    pub term: u64,
    pub leader_id: NodeId,
    pub prev_log_index: u64,
    pub prev_log_term: u64,
    pub entries: Vec<LogEntry>,
    pub leader_commit: u64,
}

/// Response to append entries
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppendEntriesResponse {
    pub term: u64,
    pub success: bool,
    pub match_index: u64,
}

/// Log entry for state machine replication
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub index: u64,
    pub term: u64,
    pub command: LogCommand,
}

/// Commands that can be logged
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogCommand {
    /// Register a client to this node
    RegisterClient {
        client_id: u128,
        titles: Vec<String>,
    },
    /// Unregister a client
    UnregisterClient { client_id: u128 },
    /// Register a title
    RegisterTitle { client_id: u128, title: String },
    /// Unregister a title
    UnregisterTitle { client_id: u128, title: String },
    /// No-op for heartbeat
    Noop,
}

/// Forward message for cross-node message delivery
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

/// Forward acknowledgment message
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

/// Title registration message for cluster propagation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleRegisterMessage {
    /// Node that registered the title
    pub node_id: String,
    /// Registered title
    pub title: String,
    /// Client ID that registered (optional, 0 if broadcast)
    pub client_id: u128,
}

/// Title unregistration message for cluster propagation
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TitleUnregisterMessage {
    /// Node that unregistered the title
    pub node_id: String,
    /// Unregistered title
    pub title: String,
}

/// Request for state synchronization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSyncRequest {
    pub request_id: u64,
    pub sync_type: SyncType,
    pub last_version: u64,
}

/// Type of state sync
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SyncType {
    /// Full state sync
    Full,
    /// Incremental sync since version
    Incremental { from_version: u64 },
}

/// Response to state sync request
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateSyncResponse {
    pub request_id: u64,
    pub version: u64,
    pub entries: Vec<StateEntry>,
}

/// A single state entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateEntry {
    pub key: String,
    pub value: Vec<u8>,
    pub operation: StateOperation,
}

/// State operation type
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateOperation {
    Put,
    Delete,
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
