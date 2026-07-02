use std::sync::Arc;

use rex_core::{RexClientInner, utils::now_secs};
use rex_persistence::{PersistenceStore, StoreConfig};
use tracing::{info, warn};

use crate::RexSystemConfig;
use crate::Shutdown;
use crate::cluster::server_cluster::ServerClusterManager;
use crate::system::ack::{AckTracker, AckTrackerImpl, PendingAckInfo};
use crate::system::client_registry::{ClientRegistry, ClientRegistryImpl};

pub struct RexSystem {
    pub config: RexSystemConfig,
    /// Client registry port (commit 4 wiring). Owns the in-memory id and title
    /// maps that handlers query. Replaces the previous `id2client` and
    /// `title2clients` fields; existing methods now delegate here.
    registry: Arc<dyn ClientRegistry>,
    shutdown: Arc<Shutdown>,
    // Persistence
    persistence: Option<Arc<PersistenceStore>>,
    // ACK tracking — port added in commit 3
    acks: Arc<dyn AckTracker>,
    // Cluster manager
    cluster_manager: parking_lot::RwLock<Option<Arc<ServerClusterManager>>>,
}

impl RexSystem {
    pub async fn new(config: RexSystemConfig, shutdown: Arc<Shutdown>) -> Arc<Self> {
        // Initialize persistence store
        let persistence = if config.persistence_enabled {
            let store_config = StoreConfig {
                path: config.persistence_path.clone(),
                enable_offline_queue: config.offline_enabled,
                enable_client_persistence: true,
                sync_interval: 1000,
            };
            match PersistenceStore::open(store_config).await {
                Ok(store) => Some(Arc::new(store)),
                Err(e) => {
                    warn!(
                        "Failed to open persistence store: {}, continuing without persistence",
                        e
                    );
                    None
                }
            }
        } else {
            None
        };

        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        let acks: Arc<dyn AckTracker> = AckTrackerImpl::new(config.ack_timeout);

        Arc::new(Self {
            config,
            registry,
            shutdown: shutdown.clone(),
            persistence,
            acks,
            cluster_manager: parking_lot::RwLock::new(None),
        })
    }

    /// Public accessor for the client registry port. The Janitor uses this
    /// to call `take_inactive` for cleanup.
    pub fn registry(&self) -> Arc<dyn ClientRegistry> {
        self.registry.clone()
    }

    /// Public accessor for the ACK tracker port. The Janitor uses this to
    /// call `take_expired` for cleanup.
    pub fn acks(&self) -> Arc<dyn AckTracker> {
        self.acks.clone()
    }

    /// Set cluster manager
    pub async fn set_cluster_manager(&self, manager: Arc<ServerClusterManager>) {
        *self.cluster_manager.write() = Some(manager);
    }

    /// Get cluster manager
    pub fn cluster_manager(&self) -> Option<Arc<ServerClusterManager>> {
        self.cluster_manager.read().clone()
    }

    /// Check if cluster is enabled
    pub fn is_cluster_enabled(&self) -> bool {
        self.cluster_manager
            .read()
            .as_ref()
            .map(|c| c.is_enabled())
            .unwrap_or(false)
    }

    /// Find node for a title (via consistent hash)
    pub fn find_node_for_title(&self, title: &str) -> Option<String> {
        if let Some(ref cluster) = self.cluster_manager() {
            // Get node for title from route table
            cluster.get_node_for_title(title)
        } else {
            None
        }
    }

    /// Get local node ID
    pub fn get_local_node_id(&self) -> Option<String> {
        self.cluster_manager()
            .as_ref()
            .map(|cluster| cluster.local_node_id().to_string())
    }

    /* ---------------- client lifecycle ---------------- */

    pub async fn add_client(&self, client: Arc<RexClientInner>) {
        let id = client.id();
        self.registry.add_client(client.clone());

        // Register client in cluster route table
        if let Some(ref cluster) = self.cluster_manager() {
            cluster.register_client(id);
        }

        // Save client state to persistence
        self.save_client_state(&client).await;
    }

    pub async fn remove_client(&self, client_id: u128) {
        let client = match self.registry.remove_client(client_id) {
            Some(client) => client,
            None => return,
        };

        // Unregister client from cluster route table
        if let Some(ref cluster) = self.cluster_manager() {
            cluster.unregister_client(&client_id);
        }

        if let Err(e) = client.close().await {
            warn!("close client [{:032X}] error: {}", client_id, e);
        } else {
            info!("client [{:032X}] removed", client_id);
        }

        // Remove client state from persistence
        self.remove_client_state(client_id).await;
    }

    pub fn register_title(&self, client_id: u128, title: &str) {
        self.registry.register_title(client_id, title);
    }

    pub fn unregister_title(&self, client_id: u128, title: &str) {
        self.registry.unregister_title(client_id, title);
    }

    /* ---------------- query ---------------- */

    pub fn find_all(&self) -> Vec<Arc<RexClientInner>> {
        self.registry.find_all()
    }

    pub fn find_all_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Vec<Arc<RexClientInner>> {
        self.registry.find_all_by_title(title, exclude)
    }

    pub fn find_one_by_title(
        &self,
        title: &str,
        exclude: Option<u128>,
    ) -> Option<Arc<RexClientInner>> {
        self.registry.find_one_by_title(title, exclude)
    }

    pub fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>> {
        self.registry.find_some_by_id(id)
    }

    /* ---------------- persistence ---------------- */

    /// Get persistence store reference
    pub fn persistence(&self) -> Option<&Arc<PersistenceStore>> {
        self.persistence.as_ref()
    }

    /// Check if persistence is enabled
    pub fn is_persistence_enabled(&self) -> bool {
        self.persistence.is_some()
    }

    /// Save client state to persistence
    pub async fn save_client_state(&self, client: &Arc<RexClientInner>) {
        if let Some(ref store) = self.persistence {
            let state = rex_persistence::ClientState::new(
                client.id(),
                client.title_iter(),
                client.local_addr().to_string(),
            );
            if let Err(e) = store.save_client(&state).await {
                warn!("Failed to save client state: {}", e);
            }
        }
    }

    /// Remove client state from persistence
    pub async fn remove_client_state(&self, client_id: u128) {
        if let Some(ref store) = self.persistence
            && let Err(e) = store.remove_client(client_id).await
        {
            warn!("Failed to remove client state: {}", e);
        }
    }

    /* ---------------- offline messages ---------------- */

    /// Queue message for offline client
    pub async fn queue_offline_message(
        &self,
        target_client_id: u128,
        title: &str,
        payload: bytes::Bytes,
    ) {
        if let Some(ref store) = self.persistence {
            let msg =
                rex_persistence::OfflineMessage::new(target_client_id, title.to_string(), payload);
            if let Err(e) = store.add_offline_message(&msg).await {
                warn!("Failed to queue offline message: {}", e);
            }
        }
    }

    /// Get offline messages for a client (and clear them)
    pub async fn get_offline_messages(
        &self,
        client_id: u128,
    ) -> Vec<rex_persistence::OfflineMessage> {
        if let Some(ref store) = self.persistence
            && let Ok(messages) = store.get_offline_messages(client_id).await
        {
            return messages;
        }
        Vec::new()
    }

    /// Clear offline messages for a client
    pub async fn clear_offline_messages(&self, client_id: u128) {
        if let Some(ref store) = self.persistence
            && let Err(e) = store.clear_offline_messages(client_id).await
        {
            warn!("Failed to clear offline messages: {}", e);
        }
    }

    /// Get offline message count for a client
    pub async fn get_offline_count(&self, client_id: u128) -> usize {
        if let Some(ref store) = self.persistence
            && let Ok(count) = store.get_offline_count(client_id).await
        {
            return count;
        }
        0
    }

    /* ---------------- ACK tracking ---------------- */

    /// Check if ACK is enabled
    pub fn is_ack_enabled(&self) -> bool {
        self.config.ack_enabled
    }

    /// Register a pending ACK
    pub fn register_pending_ack(
        &self,
        message_id: u64,
        source_client_id: u128,
        title: String,
        is_group: bool,
    ) {
        if !self.config.ack_enabled {
            return;
        }
        self.acks
            .register(message_id, source_client_id, title, is_group);
    }

    /// Get and remove pending ACK info
    pub fn take_pending_ack(&self, message_id: u64) -> Option<PendingAckInfo> {
        self.acks.take(message_id)
    }

    /// Get pending ACK info without removing
    pub fn get_pending_ack(&self, message_id: u64) -> Option<PendingAckInfo> {
        self.acks.get(message_id)
    }

    /* ---------------- shutdown ---------------- */

    pub async fn close(&self) {
        self.shutdown.signal();

        for client in self.registry.find_all() {
            if let Err(e) = client.close().await {
                warn!("close client error: {}", e);
            }
        }

        // Close persistence store
        if let Some(ref store) = self.persistence
            && let Err(e) = store.close().await
        {
            warn!("Error closing persistence store: {}", e);
        }
    }
}
