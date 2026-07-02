//! Services bundle — the single bag every long-running task holds.
//!
//! Composed of the four state ports (`ClientRegistry`, `AckTracker`,
//! `OfflineBuffer`, `ClusterPort`) plus `Shutdown`. Constructed once in
//! `lib.rs::open_server` and shared by `ServerBase`, the transports, the
//! `Janitor`, and the handler dispatch.
//!
//! The struct also exposes a few composite methods (`add_client`,
//! `remove_client`, ack-gated helpers) that orchestrate multiple ports
//! together. Handlers call these instead of repeating the cluster-handshake
//! + persistence dance in every site.

use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use rex_core::RexClientInner;
use rex_persistence::OfflineMessage;
use tracing::warn;

use crate::RexSystemConfig;
use crate::Shutdown;
use crate::system::ack::AckTracker;
use crate::system::client_registry::ClientRegistry;
use crate::system::cluster_port::ClusterPort;
use crate::system::offline::OfflineBuffer;
use crate::system::router::{ClusterRouter, Router};

pub struct Services {
    /// In-memory client/title maps. Owns the canonical id and title state.
    pub registry: Arc<dyn ClientRegistry>,

    /// Pending ACK tracker. Pure state; the Janitor delivers timeouts.
    pub acks: Arc<dyn AckTracker>,

    /// Sled-backed offline buffer (or no-op). Persists client state and
    /// queues messages for offline targets.
    pub offline: Arc<dyn OfflineBuffer>,

    /// Cluster route table + forward channel (for non-routing cluster ops).
    pub cluster: Arc<dyn ClusterPort>,

    /// Title routing — local-first then cluster-fallback. Composes the
    /// registry and cluster port (added in C4).
    pub router: Arc<dyn Router>,

    /// Cross-cutting shutdown signal — held by every long-running task.
    pub shutdown: Arc<Shutdown>,

    /// System configuration. Kept here so the composite methods (`ack_enabled`,
    /// etc.) and the handler gate checks can read it without a separate
    /// `RexSystem` reference.
    pub config: RexSystemConfig,
}

impl Services {
    pub fn new(
        registry: Arc<dyn ClientRegistry>,
        acks: Arc<dyn AckTracker>,
        offline: Arc<dyn OfflineBuffer>,
        cluster: Arc<dyn ClusterPort>,
        router: Arc<dyn Router>,
        shutdown: Arc<Shutdown>,
        config: RexSystemConfig,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            acks,
            offline,
            cluster,
            router,
            shutdown,
            config,
        })
    }

    /* ---------------- composite ops (multi-port) ---------------- */

    /// Register a freshly-connected client. Updates the registry, notifies
    /// the cluster route table, and persists client state.
    pub async fn add_client(&self, client: Arc<RexClientInner>) {
        let id = client.id();
        self.registry.add_client(client.clone());
        self.cluster.register_client(id);
        self.offline.save_client(&client).await;
    }

    /// Remove a client. Returns the removed client so the caller can do
    /// post-removal work (e.g. log the address). Updates the registry,
    /// notifies cluster, closes the connection, and removes persistence.
    pub async fn remove_client(&self, client_id: u128) -> Option<Arc<RexClientInner>> {
        let client = self.registry.remove_client(client_id)?;
        self.cluster.unregister_client(client_id);
        if let Err(e) = client.close().await {
            warn!("close client [{:032X}] error: {}", client_id, e);
        } else {
            tracing::info!("client [{:032X}] removed", client_id);
        }
        self.offline.remove_client(client_id).await;
        Some(client)
    }

    /// Register a pending ACK. No-op when `ack_enabled` is false.
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

    pub fn take_pending_ack(&self, message_id: u64) -> Option<crate::system::ack::PendingAckInfo> {
        self.acks.take(message_id)
    }

    pub fn get_pending_ack(&self, message_id: u64) -> Option<crate::system::ack::PendingAckInfo> {
        self.acks.get(message_id)
    }

    /// Queue a message for an offline target.
    pub async fn queue_offline_message(&self, target_client_id: u128, title: &str, payload: Bytes) {
        self.offline
            .queue_offline_message(target_client_id, title, payload)
            .await;
    }

    /// Forwarded to the offline port.
    pub async fn get_offline_messages(&self, client_id: u128) -> Vec<OfflineMessage> {
        self.offline.get_offline_messages(client_id).await
    }

    pub async fn clear_offline_messages(&self, client_id: u128) {
        self.offline.clear_offline_messages(client_id).await;
    }

    /* ---------------- accessors / config-driven flags ---------------- */

    pub fn is_ack_enabled(&self) -> bool {
        self.config.ack_enabled
    }

    pub fn is_persistence_enabled(&self) -> bool {
        self.config.persistence_enabled
    }

    /// Convenience: serialise the rex_data acknowledgement and send it.
    /// Used by ack.rs.
    pub async fn send_ack(
        &self,
        sender: &Arc<RexClientInner>,
        message_id: u64,
        retcode: rex_core::RetCode,
        command: rex_core::RexCommand,
        source_client_id: u128,
    ) -> Result<()> {
        let ack_data = rex_core::AckData::new(message_id);
        let rex_data = ack_data.to_rex_data(source_client_id, command);
        let mut rex_data = rex_data;
        rex_data.set_retcode(retcode);
        sender.send_buf(rex_data.pack_ref()).await?;
        Ok(())
    }
}
