use std::sync::Arc;

use rex_core::RexClientInner;
use tracing::{info, warn};

use crate::RexSystemConfig;
use crate::Shutdown;
use crate::cluster::server_cluster::ServerClusterManager;
use crate::system::ack::{AckTracker, AckTrackerImpl, PendingAckInfo};
use crate::system::client_registry::{ClientRegistry, ClientRegistryImpl};
use crate::system::offline::{NoopOfflineBuffer, OfflineBuffer, SledOfflineBuffer};

pub struct RexSystem {
    pub config: RexSystemConfig,
    /// Client registry port (commit 4 wiring). Owns the in-memory id and title
    /// maps that handlers query. Replaces the previous `id2client` and
    /// `title2clients` fields; existing methods now delegate here.
    registry: Arc<dyn ClientRegistry>,
    pub shutdown: Arc<Shutdown>,
    // Offline buffer + client-state persistence (port added in commit 5).
    // Noop when persistence is disabled; Sled-backed otherwise.
    offline: Arc<dyn OfflineBuffer>,
    // ACK tracking — port added in commit 3
    acks: Arc<dyn AckTracker>,
    // Cluster manager
    cluster_manager: parking_lot::RwLock<Option<Arc<ServerClusterManager>>>,
}

impl RexSystem {
    pub async fn new(config: RexSystemConfig, shutdown: Arc<Shutdown>) -> Arc<Self> {
        // Initialize offline buffer. Sled-backed when persistence is enabled,
        // no-op otherwise. Errors fall back to no-op so the server starts
        // even if the sled path is unwritable.
        let offline: Arc<dyn OfflineBuffer> = if config.persistence_enabled {
            match SledOfflineBuffer::open(config.persistence_path.clone()).await {
                Ok(buf) => buf,
                Err(e) => {
                    warn!(
                        "Failed to open persistence store: {}, continuing without persistence",
                        e
                    );
                    Arc::new(NoopOfflineBuffer)
                }
            }
        } else {
            Arc::new(NoopOfflineBuffer)
        };

        let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
        let acks: Arc<dyn AckTracker> = AckTrackerImpl::new(config.ack_timeout);

        Arc::new(Self {
            config,
            registry,
            shutdown: shutdown.clone(),
            offline,
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

    /// Public accessor for the offline-buffer port. Login drains queued
    /// messages through this; the cleanup loop (future) deletes expired
    /// ones through it.
    pub fn offline(&self) -> Arc<dyn OfflineBuffer> {
        self.offline.clone()
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

        // Save client state via the offline port (commit 5).
        self.offline.save_client(&client).await;
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

        // Remove client state via the offline port (commit 5).
        self.offline.remove_client(client_id).await;
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

    /// Whether persistence is configured to be on. The offline port may still
    /// fall back to NoopOfflineBuffer if sled failed to open at startup.
    pub fn is_persistence_enabled(&self) -> bool {
        self.config.persistence_enabled
    }

    /* ---------------- offline messages ---------------- */

    /// Queue a message for an offline target. Forwarded to the offline port.
    pub async fn queue_offline_message(
        &self,
        target_client_id: u128,
        title: &str,
        payload: bytes::Bytes,
    ) {
        self.offline
            .queue_offline_message(target_client_id, title, payload)
            .await;
    }

    /// Get queued messages for a client. Forwarded to the offline port.
    pub async fn get_offline_messages(
        &self,
        client_id: u128,
    ) -> Vec<rex_persistence::OfflineMessage> {
        self.offline.get_offline_messages(client_id).await
    }

    /// Clear queued messages for a client. Forwarded to the offline port.
    pub async fn clear_offline_messages(&self, client_id: u128) {
        self.offline.clear_offline_messages(client_id).await;
    }

    /// Get queued message count for a client. Forwarded to the offline port.
    pub async fn get_offline_count(&self, client_id: u128) -> usize {
        self.offline.get_offline_count(client_id).await
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

        // Flush + close the offline port (sled is reference-counted; its
        // drop would flush too, but explicit close is clearer).
        self.offline.close().await;
    }
}
