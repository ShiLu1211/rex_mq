//! Node Manager Manages cluster nodes
//!
//! , discovery, and state

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};

use crate::transport::{ClusterTransport, IncomingMessage};
use crate::types::{
    ClusterConfig, ClusterMessage, ClusterRole, HeartbeatMessage, NodeId, NodeInfo,
};

/// Cluster node manager
#[allow(dead_code)]
pub struct NodeManager {
    /// Local node configuration
    config: ClusterConfig,
    /// Current role
    role: RwLock<ClusterRole>,
    /// Current term (for Raft)
    term: RwLock<u64>,
    /// Voted for (candidate_id)
    voted_for: RwLock<Option<NodeId>>,
    /// Votes received in current election (candidate_id -> bool)
    votes_received: RwLock<std::collections::HashMap<String, bool>>,
    /// Known nodes (node_id -> NodeInfo)
    nodes: DashMap<String, NodeInfo>,
    /// Last time we heard from leader
    last_leader_contact: RwLock<u64>,
    /// Transport layer
    transport: Arc<ClusterTransport>,
    /// Message handler channel
    message_tx: mpsc::UnboundedSender<ClusterMessage>,
    /// Shutdown signal
    shutdown_tx: broadcast::Sender<()>,
}

impl NodeManager {
    /// Create a new node manager
    pub fn new(config: ClusterConfig, message_tx: mpsc::UnboundedSender<ClusterMessage>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let local_node_id = config.node_id.clone();

        let (transport_tx, transport_rx) = mpsc::unbounded_channel();

        let transport = Arc::new(ClusterTransport::new(local_node_id.clone(), transport_tx));

        // Spawn message forwarding task
        let nodes_map = Arc::new(DashMap::new());
        let nodes_map_clone = nodes_map.clone();
        let message_tx_clone = message_tx.clone();

        tokio::spawn(async move {
            Self::forward_messages(transport_rx, nodes_map_clone, message_tx_clone).await;
        });

        Self {
            config,
            role: RwLock::new(ClusterRole::Standalone),
            term: RwLock::new(0),
            voted_for: RwLock::new(None),
            votes_received: RwLock::new(std::collections::HashMap::new()),
            nodes: DashMap::new(),
            last_leader_contact: RwLock::new(0),
            transport,
            message_tx,
            shutdown_tx,
        }
    }

    /// Forward incoming messages to the appropriate handler
    async fn forward_messages(
        mut rx: mpsc::UnboundedReceiver<IncomingMessage>,
        nodes: Arc<DashMap<String, NodeInfo>>,
        message_tx: mpsc::UnboundedSender<ClusterMessage>,
    ) {
        while let Some(incoming) = rx.recv().await {
            let source_node_id = incoming.source_node.as_str().to_string();

            // Update last seen time for the node
            if let Some(mut node_info) = nodes.get_mut(&source_node_id) {
                node_info.last_heartbeat = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
            }

            // Forward to message handler
            if let Err(e) = message_tx.send(incoming.message) {
                // This happens when the receiver is dropped (e.g., during shutdown)
                debug!("Failed to forward message (receiver dropped): {}", e);
            }
        }
    }

    /// Start the node manager
    pub async fn start(&self) -> Result<()> {
        // Start transport listener
        let listen_addr = self.config.listen_addr;
        let transport = self.transport.clone();

        tokio::spawn(async move {
            if let Err(e) = transport.start_listener(listen_addr).await {
                error!("Transport listener error: {}", e);
            }
        });

        // Connect to seed nodes if any
        if !self.config.seed_nodes.is_empty() {
            self.connect_to_seeds().await?;
        }

        // Start heartbeat/task loop
        self.start_background_tasks();

        Ok(())
    }

    /// Connect to seed nodes
    async fn connect_to_seeds(&self) -> Result<()> {
        for seed_addr in &self.config.seed_nodes {
            // Use the seed address as a temporary node_id key since we don't know the seed's actual node_id yet
            // After receiving the NodeList response, we'll update the route table with the correct node_id
            let temp_node_id = format!("seed-{}", seed_addr);
            info!(
                "Connecting to seed at {} (temp key: {})",
                seed_addr, temp_node_id
            );

            if let Err(e) = self
                .transport
                .connect(temp_node_id.into(), *seed_addr)
                .await
            {
                warn!("Failed to connect to seed {}: {}", seed_addr, e);
                continue;
            }

            // Send Join message with our real node_id
            let local_info = NodeInfo::new(self.config.node_id.clone(), self.config.listen_addr);
            let join_msg = ClusterMessage::Join(local_info);
            let temp_key = format!("seed-{}", seed_addr);

            if let Err(e) = self.transport.send_to(&temp_key, &join_msg).await {
                warn!("Failed to send join to seed {}: {}", seed_addr, e);
            }
        }

        Ok(())
    }

    /// Start background tasks (heartbeat, etc.)
    fn start_background_tasks(&self) {
        let heartbeat_interval = Duration::from_millis(self.config.heartbeat_interval_ms);
        let role = Arc::new(parking_lot::RwLock::new(*self.role.read()));
        let term = Arc::new(parking_lot::RwLock::new(*self.term.read()));
        let _nodes = Arc::new(self.nodes.clone());
        let transport = self.transport.clone();
        let config = self.config.clone();

        // Create mutable references for the background task
        let role_ref = role.clone();
        let term_ref = term.clone();

        tokio::spawn(async move {
            let mut ticker = interval(heartbeat_interval);

            loop {
                ticker.tick().await;

                let current_role = *role_ref.read();

                match current_role {
                    ClusterRole::Leader => {
                        // Send heartbeat to all followers
                        let heartbeat = HeartbeatMessage {
                            term: *term_ref.read(),
                            leader_id: config.node_id.clone(),
                            leader_commit: 0,
                        };
                        let msg = ClusterMessage::Heartbeat(heartbeat);

                        if let Err(e) = transport.broadcast(&msg).await {
                            debug!("Heartbeat broadcast error: {}", e);
                        }
                    }
                    ClusterRole::Candidate => {
                        // Request votes from other nodes
                        let term_val = *term_ref.read() + 1;
                        *term_ref.write() = term_val;

                        let vote_msg = crate::types::RequestVoteMessage {
                            term: term_val,
                            candidate_id: config.node_id.clone(),
                            last_log_index: 0,
                            last_log_term: 0,
                        };

                        let msg = ClusterMessage::RequestVote(vote_msg);

                        if let Err(e) = transport.broadcast(&msg).await {
                            debug!("Vote request broadcast error: {}", e);
                        }
                    }
                    ClusterRole::Follower | ClusterRole::Standalone => {
                        // Do nothing, wait for messages
                    }
                }
            }
        });
    }

    /// Handle an incoming cluster message
    pub async fn handle_message(&self, message: ClusterMessage) -> Result<()> {
        match message {
            ClusterMessage::Join(node_info) => {
                self.handle_join(node_info).await?;
            }
            ClusterMessage::Heartbeat(heartbeat) => {
                self.handle_heartbeat(heartbeat).await?;
            }
            ClusterMessage::RequestVote(vote_req) => {
                self.handle_request_vote(vote_req).await?;
            }
            ClusterMessage::VoteResponse(vote_resp) => {
                self.handle_vote_response_msg(vote_resp).await?;
            }
            ClusterMessage::AppendEntries(append_req) => {
                self.handle_append_entries(append_req).await?;
            }
            ClusterMessage::AppendEntriesResponse(resp) => {
                self.handle_append_entries_response(resp).await?;
            }
            ClusterMessage::Forward(forward) => {
                self.handle_forward(forward).await?;
            }
            _ => {
                debug!("Unhandled cluster message: {:?}", message);
            }
        }

        Ok(())
    }

    /// Handle node join
    async fn handle_join(&self, node_info: NodeInfo) -> Result<()> {
        let node_id_str = node_info.node_id.as_str().to_string();

        info!("Node {} joined the cluster", node_id_str);

        // Add or update node
        self.nodes.insert(node_id_str.clone(), node_info);

        // If we're the leader, send current state
        if *self.role.read() == ClusterRole::Leader {
            // TODO: Send state sync
        }

        Ok(())
    }

    /// Handle heartbeat from leader
    async fn handle_heartbeat(&self, heartbeat: HeartbeatMessage) -> Result<()> {
        // Update term
        if heartbeat.term > *self.term.read() {
            *self.term.write() = heartbeat.term;
            *self.role.write() = ClusterRole::Follower;
            *self.voted_for.write() = None;
        }

        // Reset election timeout
        // In real implementation, we'd reset a timer here

        Ok(())
    }

    /// Handle vote request
    async fn handle_request_vote(&self, vote_req: crate::types::RequestVoteMessage) -> Result<()> {
        // Compute the vote decision while holding the locks, then drop the
        // guards BEFORE any `.await` — parking_lot's sync RwLock guards
        // must not span await points (clippy::await_holding_lock).
        let (term_value, vote_granted) = {
            let mut term = self.term.write();
            let mut role = self.role.write();
            let mut voted_for = self.voted_for.write();

            let mut vote_granted = false;

            // Vote for candidate if:
            // 1. Candidate's term >= our term
            // 2. We haven't voted for anyone, or we've voted for this candidate
            // 3. Candidate's log is at least as up-to-date as ours
            if vote_req.term >= *term {
                if voted_for.is_none() || voted_for.as_ref() == Some(&vote_req.candidate_id) {
                    vote_granted = true;
                    *voted_for = Some(vote_req.candidate_id.clone());
                }

                // Update our term
                *term = vote_req.term;

                // Become follower
                *role = ClusterRole::Follower;
            }

            (*term, vote_granted)
        };

        // Send vote response
        let vote_resp = crate::types::VoteResponseMessage {
            term: term_value,
            vote_granted,
            voter_id: self.config.node_id.clone(),
        };

        let msg = ClusterMessage::VoteResponse(vote_resp);

        if let Err(e) = self
            .transport
            .send_to(vote_req.candidate_id.as_str(), &msg)
            .await
        {
            warn!("Failed to send vote response: {}", e);
        }

        Ok(())
    }

    /// Handle vote response from another node
    async fn handle_vote_response_msg(
        &self,
        vote_resp: crate::types::VoteResponseMessage,
    ) -> Result<()> {
        self.handle_vote_response(&vote_resp.voter_id, vote_resp.vote_granted);
        Ok(())
    }

    /// Handle append entries (log replication)
    async fn handle_append_entries(
        &self,
        _append_req: crate::types::AppendEntriesMessage,
    ) -> Result<()> {
        // TODO: Implement log replication
        Ok(())
    }

    /// Handle append entries response
    async fn handle_append_entries_response(
        &self,
        _resp: crate::types::AppendEntriesResponse,
    ) -> Result<()> {
        // TODO: Handle replication success/failure
        Ok(())
    }

    /// Handle forwarded message (cross-node message delivery)
    async fn handle_forward(&self, forward: crate::types::ForwardMessage) -> Result<()> {
        // Forward to the message handler for delivery to local clients
        // This would be integrated with rex-server
        debug!(
            "Received forwarded message for client {}",
            forward.target_client_id
        );

        Ok(())
    }

    /// Get current role
    pub fn role(&self) -> ClusterRole {
        *self.role.read()
    }

    /// Get current term
    pub fn term(&self) -> u64 {
        *self.term.read()
    }

    /// Get local node ID
    pub fn node_id(&self) -> &NodeId {
        &self.config.node_id
    }

    /// Get all known nodes
    pub fn nodes(&self) -> Vec<NodeInfo> {
        self.nodes.iter().map(|e| e.value().clone()).collect()
    }

    /// Check if this node is leader
    pub fn is_leader(&self) -> bool {
        *self.role.read() == ClusterRole::Leader
    }

    /// Start an election
    pub fn start_election(&self) {
        // Increment term
        let mut term = self.term.write();
        *term += 1;
        let current_term = *term;
        drop(term);

        // Become candidate
        *self.role.write() = ClusterRole::Candidate;

        // Vote for self
        *self.voted_for.write() = Some(self.config.node_id.clone());

        // Reset votes
        self.votes_received.write().clear();
        self.votes_received
            .write()
            .insert(self.config.node_id.as_str().to_string(), true);

        info!(
            "Starting election for term {}, node {}",
            current_term,
            self.config.node_id.as_str()
        );

        // Request votes from other nodes
        let vote_msg = crate::types::RequestVoteMessage {
            term: current_term,
            candidate_id: self.config.node_id.clone(),
            last_log_index: 0,
            last_log_term: 0,
        };

        let msg = ClusterMessage::RequestVote(vote_msg);

        let transport = self.transport.clone();
        tokio::spawn(async move {
            if let Err(e) = transport.broadcast(&msg).await {
                debug!("Failed to send vote requests: {}", e);
            }
        });
    }

    /// Handle vote response and check if we won
    pub fn handle_vote_response(&self, voter_id: &NodeId, vote_granted: bool) {
        if !vote_granted {
            return;
        }

        let mut votes = self.votes_received.write();
        votes.insert(voter_id.as_str().to_string(), true);

        // Check if we have majority
        let total_nodes = self.nodes.len() + 1; // +1 for self
        let majority = (total_nodes / 2) + 1;
        let vote_count = votes.values().filter(|&&v| v).count();

        if vote_count >= majority {
            info!(
                "Won election with {} votes (majority: {})",
                vote_count, majority
            );
            *self.role.write() = ClusterRole::Leader;
        }
    }

    /// Update last contact with leader
    pub fn update_leader_contact(&self) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        *self.last_leader_contact.write() = now;
    }

    /// Check if we should start an election (leader timeout)
    pub fn check_election_timeout(&self) -> bool {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let last_contact = *self.last_leader_contact.read();
        let elapsed = now.saturating_sub(last_contact);

        // Check if we're not leader and haven't heard from leader
        if !self.is_leader() && elapsed > self.config.election_timeout_min_ms {
            return true;
        }
        false
    }

    /// Send a message to another node
    pub async fn send_to(&self, target_node_id: &str, message: ClusterMessage) -> Result<()> {
        self.transport.send_to(target_node_id, &message).await
    }

    /// Get the transport layer
    pub fn get_transport(&self) -> Arc<ClusterTransport> {
        self.transport.clone()
    }

    /// Shutdown the node manager
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
        self.transport.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_manager_creation() {
        let config = ClusterConfig::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        let manager = NodeManager::new(config, tx);

        assert_eq!(manager.role(), ClusterRole::Standalone);
    }
}
