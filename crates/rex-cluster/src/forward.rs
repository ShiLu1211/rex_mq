//! Message Forwarder
//!
//! Handles cross-node message forwarding

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use parking_lot::RwLock;
use tokio::sync::mpsc;
use tracing::{debug, error, warn};

use crate::route_table::GlobalRouteTable;
use crate::transport::ClusterTransport;
use crate::types::{ClusterMessage, ForwardMessage, NodeId};

/// Message forwarder for cross-node communication
pub struct MessageForwarder {
    /// Transport layer
    transport: Arc<ClusterTransport>,
    /// Route table
    route_table: Arc<GlobalRouteTable>,
    /// Local node ID
    local_node_id: NodeId,
    /// Pending forwards for ACK tracking
    pending_forwards: DashMap<u64, PendingForward>,
    /// Forward ID counter
    forward_id_counter: RwLock<u64>,
    /// Message callback (for delivering to local clients)
    message_callback: RwLock<Option<mpsc::UnboundedSender<ForwardedMessage>>>,
}

/// Pending forward for tracking
#[allow(dead_code)]
#[derive(Debug, Clone)]
struct PendingForward {
    forward_id: u64,
    target_node: String,
    original_source: u128,
    title: String,
    created_at: u64,
}

/// Message delivered from another node
#[derive(Debug, Clone)]
pub struct ForwardedMessage {
    /// Original source client ID
    pub source_client_id: u128,
    /// Target client ID
    pub target_client_id: u128,
    /// Message title
    pub title: String,
    /// Message payload
    pub payload: Vec<u8>,
    /// Whether this is a group message
    pub is_group: bool,
    /// Whether this is a broadcast
    pub is_broadcast: bool,
    /// Whether ACK is required
    pub require_ack: bool,
    /// Forward ID for tracking
    pub forward_id: u64,
}

#[allow(dead_code)]
impl MessageForwarder {
    /// Create a new message forwarder
    pub fn new(
        transport: Arc<ClusterTransport>,
        route_table: Arc<GlobalRouteTable>,
        local_node_id: NodeId,
    ) -> Self {
        Self {
            transport,
            route_table,
            local_node_id,
            pending_forwards: DashMap::new(),
            forward_id_counter: RwLock::new(0),
            message_callback: RwLock::new(None),
        }
    }

    /// Set message callback for delivering to local clients
    pub fn set_message_callback(&self, callback: mpsc::UnboundedSender<ForwardedMessage>) {
        *self.message_callback.write() = Some(callback);
    }

    /// Forward a message to another node
    pub async fn forward_message(
        &self,
        original_source: u128,
        target_client_id: u128,
        title: String,
        payload: Vec<u8>,
        is_group: bool,
        is_broadcast: bool,
        require_ack: bool,
    ) -> Result<Option<u64>> {
        // Find target node
        let target_node = self.route_table.get_node_by_client(&target_client_id);

        // If no node found, try title-based routing
        let target_node = match target_node {
            Some(node) => node,
            None => {
                // Fallback to title-based routing
                match self.route_table.get_node_by_title(&title) {
                    Some(node) => node,
                    None => {
                        debug!(
                            "No route found for client {} or title {}",
                            target_client_id, title
                        );
                        return Ok(None);
                    }
                }
            }
        };

        // Skip if target is local node
        if target_node == self.local_node_id.as_str() {
            // Deliver locally
            self.deliver_locally(
                original_source,
                target_client_id,
                title,
                payload,
                is_group,
                is_broadcast,
                require_ack,
                0,
            )
            .await?;
            return Ok(None);
        }

        // Generate forward ID
        let forward_id = {
            let mut counter = self.forward_id_counter.write();
            *counter += 1;
            *counter
        };

        // Create forward message
        let forward = ForwardMessage {
            forward_id,
            original_source,
            target_client_id,
            title: title.clone(),
            payload: payload.clone(),
            is_group,
            is_broadcast,
            require_ack,
        };

        // Track pending forward
        if require_ack {
            let pending = PendingForward {
                forward_id,
                target_node: target_node.clone(),
                original_source,
                title: title.clone(),
                created_at: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            };
            self.pending_forwards.insert(forward_id, pending);
        }

        // Send to target node
        let msg = ClusterMessage::Forward(forward);

        match self.transport.send_to(&target_node, &msg).await {
            Ok(()) => {
                debug!(
                    "Forwarded message to node {} (forward_id: {})",
                    target_node, forward_id
                );
                Ok(Some(forward_id))
            }
            Err(e) => {
                error!("Failed to forward message to {}: {}", target_node, e);
                // Remove from pending if failed
                if require_ack {
                    self.pending_forwards.remove(&forward_id);
                }
                Err(e)
            }
        }
    }

    /// Broadcast a message to all nodes that have subscribers for a title
    pub async fn broadcast_to_nodes(
        &self,
        original_source: u128,
        title: String,
        payload: Vec<u8>,
        is_broadcast: bool,
        require_ack: bool,
    ) -> Result<Vec<String>> {
        // Get all nodes that might have subscribers for this title
        let target_nodes = self.route_table.get_nodes_by_title(&title, 3);

        let mut successful_nodes = Vec::new();

        for node_id in target_nodes {
            // Skip local node
            if node_id == self.local_node_id.as_str() {
                continue;
            }

            let forward_id = {
                let mut counter = self.forward_id_counter.write();
                *counter += 1;
                *counter
            };

            let forward = ForwardMessage {
                forward_id,
                original_source,
                target_client_id: 0, // Not specific client
                title: title.clone(),
                payload: payload.clone(),
                is_group: false,
                is_broadcast,
                require_ack,
            };

            let msg = ClusterMessage::Forward(forward);

            if let Err(e) = self.transport.send_to(&node_id, &msg).await {
                warn!("Failed to broadcast to node {}: {}", node_id, e);
            } else {
                successful_nodes.push(node_id);
            }
        }

        Ok(successful_nodes)
    }

    /// Handle an incoming forwarded message
    pub async fn handle_forwarded_message(&self, forward: ForwardMessage) -> Result<()> {
        // Deliver to local clients
        self.deliver_locally(
            forward.original_source,
            forward.target_client_id,
            forward.title,
            forward.payload,
            forward.is_group,
            forward.is_broadcast,
            forward.require_ack,
            forward.forward_id,
        )
        .await
    }

    /// Deliver a message to local clients
    async fn deliver_locally(
        &self,
        original_source: u128,
        target_client_id: u128,
        title: String,
        payload: Vec<u8>,
        is_group: bool,
        is_broadcast: bool,
        require_ack: bool,
        forward_id: u64,
    ) -> Result<()> {
        let msg = ForwardedMessage {
            source_client_id: original_source,
            target_client_id,
            title,
            payload,
            is_group,
            is_broadcast,
            require_ack,
            forward_id,
        };

        let callback = self.message_callback.read();
        if let Some(cb) = callback.as_ref()
            && let Err(e) = cb.send(msg)
        {
            error!("Failed to deliver message to local handler: {}", e);
        }

        Ok(())
    }

    /// Handle ACK for a forwarded message
    fn handle_ack(&self, forward_id: u64) -> Option<PendingForward> {
        self.pending_forwards.remove(&forward_id).map(|(_, v)| v)
    }

    /// Clean up timed out pending forwards
    fn cleanup_timeout(&self, timeout_ms: u64) -> Vec<PendingForward> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);

        let mut timed_out = Vec::new();

        self.pending_forwards.retain(|_, pending| {
            if now - pending.created_at > timeout_ms {
                timed_out.push(pending.clone());
                false // Remove
            } else {
                true // Keep
            }
        });

        timed_out
    }
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_forwarder_creation() {
        // This would require more setup, skipping for now
    }
}
