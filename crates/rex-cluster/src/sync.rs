//! State Synchronization
//!
//! Provides state synchronization between cluster nodes (Leader to Followers)

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tracing::{debug, info};

use crate::route_table::GlobalRouteTable;
use crate::transport::ClusterTransport;
use crate::types::{
    ClusterMessage, NodeId, StateEntry, StateSyncRequest, StateSyncResponse, SyncType,
};

/// State synchronizer for cluster nodes
#[allow(dead_code)]
pub struct StateSyncer {
    /// Local node ID
    local_node_id: NodeId,
    /// Transport layer
    transport: Arc<ClusterTransport>,
    /// Route table
    route_table: Arc<GlobalRouteTable>,
    /// Local state version
    local_version: RwLock<u64>,
    /// Pending sync requests
    pending_syncs: DashMap<u64, StateSyncRequest>,
    /// Sync request ID counter
    sync_id_counter: RwLock<u64>,
    /// Message sender for applying synced state
    state_apply_tx: Option<mpsc::UnboundedSender<StateUpdate>>,
}

/// State update to apply
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateUpdate {
    /// Update type
    pub update_type: StateUpdateType,
    /// Key (e.g., client_id)
    pub key: String,
    /// Value
    pub value: Option<Vec<u8>>,
}

/// Type of state update
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateUpdateType {
    /// Client registration
    ClientRegister,
    /// Client unregistration
    ClientUnregister,
    /// Title registration
    TitleRegister,
    /// Title unregistration
    TitleUnregister,
}

impl StateSyncer {
    /// Create a new state synchronizer
    pub fn new(
        local_node_id: NodeId,
        transport: Arc<ClusterTransport>,
        route_table: Arc<GlobalRouteTable>,
    ) -> Self {
        Self {
            local_node_id,
            transport,
            route_table,
            local_version: RwLock::new(0),
            pending_syncs: DashMap::new(),
            sync_id_counter: RwLock::new(0),
            state_apply_tx: None,
        }
    }

    /// Set state apply callback
    pub fn set_apply_callback(&self, _tx: mpsc::UnboundedSender<StateUpdate>) {
        // This would be used to apply state updates to local RexSystem
    }

    /// Request full state sync from leader
    pub async fn request_full_sync(&self, leader_node_id: &str) -> Result<()> {
        let sync_id = {
            let mut counter = self.sync_id_counter.write();
            *counter += 1;
            *counter
        };

        let request = StateSyncRequest {
            request_id: sync_id,
            sync_type: SyncType::Full,
            last_version: 0,
        };

        // Track pending sync
        self.pending_syncs.insert(sync_id, request.clone());

        let msg = ClusterMessage::StateSyncRequest(request);

        self.transport.send_to(leader_node_id, &msg).await?;

        info!("Requested full state sync from {}", leader_node_id);

        Ok(())
    }

    /// Request incremental state sync
    pub async fn request_incremental_sync(
        &self,
        leader_node_id: &str,
        from_version: u64,
    ) -> Result<()> {
        let sync_id = {
            let mut counter = self.sync_id_counter.write();
            *counter += 1;
            *counter
        };

        let request = StateSyncRequest {
            request_id: sync_id,
            sync_type: SyncType::Incremental { from_version },
            last_version: from_version,
        };

        self.pending_syncs.insert(sync_id, request.clone());

        let msg = ClusterMessage::StateSyncRequest(request);

        self.transport.send_to(leader_node_id, &msg).await?;

        debug!(
            "Requested incremental state sync from {} since version {}",
            leader_node_id, from_version
        );

        Ok(())
    }

    /// Handle sync request (Leader side)
    pub async fn handle_sync_request(&self, request: &StateSyncRequest) -> Result<()> {
        let state = self.export_state();

        let response = StateSyncResponse {
            request_id: request.request_id,
            version: *self.local_version.read(),
            entries: state,
        };

        let _msg = ClusterMessage::StateSyncResponse(response);

        // Send to requester
        // TODO: In real implementation, we'd track the requester and send this message

        info!(
            "Responding to sync request {} with version {}",
            request.request_id,
            *self.local_version.read()
        );

        Ok(())
    }

    /// Handle sync response (Follower side)
    pub async fn handle_sync_response(&self, response: &StateSyncResponse) -> Result<()> {
        // Remove from pending
        self.pending_syncs.remove(&response.request_id);

        // Apply state updates
        self.apply_state(&response.entries).await?;

        // Update local version
        *self.local_version.write() = response.version;

        info!("Applied state sync, new version: {}", response.version);

        Ok(())
    }

    /// Export local state for synchronization
    pub fn export_state(&self) -> Vec<StateEntry> {
        let mut entries = Vec::new();

        // Export client registrations
        let client_id_to_node = self.route_table.export_state();

        for (client_id, node_id) in client_id_to_node.client_id_to_node {
            let value =
                bincode::serialize(&StateValue::ClientRegister { node_id }).unwrap_or_default();

            entries.push(crate::types::StateEntry {
                key: format!("client:{}", client_id),
                value,
                operation: crate::types::StateOperation::Put,
            });
        }

        // Increment version
        let mut version = self.local_version.write();
        *version += 1;

        entries
    }

    /// Apply state updates from sync
    async fn apply_state(&self, entries: &[crate::types::StateEntry]) -> Result<()> {
        for entry in entries {
            match entry.operation {
                crate::types::StateOperation::Put => {
                    if let Ok(value) = bincode::deserialize::<StateValue>(&entry.value) {
                        self.apply_state_entry(&entry.key, &value).await;
                    }
                }
                crate::types::StateOperation::Delete => {
                    self.apply_state_delete(&entry.key).await;
                }
            }
        }

        Ok(())
    }

    /// Apply a single state entry
    async fn apply_state_entry(&self, key: &str, value: &StateValue) {
        match value {
            StateValue::ClientRegister { node_id } => {
                if let Some(stripped) = key.strip_prefix("client:")
                    && let Ok(client_id) = stripped.parse::<u128>()
                {
                    self.route_table.register_client(client_id, node_id);
                    debug!("Applied client registration: {} -> {}", client_id, node_id);
                }
            }
        }
    }

    /// Apply state deletion
    async fn apply_state_delete(&self, key: &str) {
        if let Some(stripped) = key.strip_prefix("client:")
            && let Ok(client_id) = stripped.parse::<u128>()
        {
            self.route_table.unregister_client(&client_id);
            debug!("Applied client deletion: {}", client_id);
        }
    }

    /// Get current state version
    pub fn version(&self) -> u64 {
        *self.local_version.read()
    }

    /// Broadcast state to all followers (Leader only)
    pub async fn broadcast_state(&self) -> Result<()> {
        let state = self.export_state();

        let response = StateSyncResponse {
            request_id: 0,
            version: *self.local_version.read(),
            entries: state,
        };

        let msg = ClusterMessage::StateSyncResponse(response);

        // Broadcast to all nodes
        self.transport.broadcast(&msg).await?;

        debug!(
            "Broadcast state to followers, version: {}",
            *self.local_version.read()
        );

        Ok(())
    }
}

/// State value for serialization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateValue {
    ClientRegister { node_id: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_value_serialization() {
        let value = StateValue::ClientRegister {
            node_id: "node1".to_string(),
        };
        let encoded = match bincode::serialize(&value) {
            Ok(e) => e,
            Err(_) => return,
        };
        let decoded: StateValue = match bincode::deserialize(&encoded) {
            Ok(d) => d,
            Err(_) => return,
        };

        match decoded {
            StateValue::ClientRegister { node_id } => {
                assert_eq!(node_id, "node1");
            }
        }
    }
}
