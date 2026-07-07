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

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use bytes::Bytes;
use dashmap::DashMap;
use parking_lot::Mutex;
use rex_core::RexClientInner;
use rex_observability::health::HealthRegistry;
use rex_persistence::OfflineMessage;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::RexSystemConfig;
use crate::Shutdown;
use crate::system::ack::AckTracker;
use crate::system::client_registry::ClientRegistry;
use crate::system::cluster_port::ClusterPort;
use crate::system::forwarder::Forwarder;
use crate::system::offline::OfflineBuffer;
#[allow(unused_imports)]
use crate::system::router::Router;

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

    /// Cross-node message delivery (outbound `forward`, inbound `deliver`,
    /// `broadcast`). Added per ADR-0002; populated in `build_services`,
    /// slots for `NodeManager` / `GlobalRouteTable` are filled by
    /// `ServerClusterManager::start`.
    pub forwarder: Arc<dyn Forwarder>,

    /// Cross-cutting shutdown signal — held by every long-running task.
    pub shutdown: Arc<Shutdown>,

    /// System configuration. Kept here so the composite methods (`ack_enabled`,
    /// etc.) and the handler gate checks can read it without a separate
    /// `RexSystem` reference.
    pub config: RexSystemConfig,

    /// Per-client cancellation signals. Created in `add_client`,
    /// cancelled by admin disconnect, awaited by the transport loop.
    pub client_shutdowns: Arc<DashMap<u128, CancellationToken>>,

    /// Health-probe registry. Probes are registered after construction
    /// in `lib.rs::open_server`. Shared with the observability admin
    /// server so `/readyz` can read the same registry the handlers see.
    pub health: Arc<HealthRegistry>,

    /// Resolved observability admin address (e.g. `/metrics` listener).
    /// Populated by `open_server` once the admin HTTP server has bound
    /// the configured port. `None` until then. Tests use this to scrape
    /// `/metrics` after publishing — the config-supplied address may
    /// be port `0` (ephemeral) and only known post-bind.
    pub admin_addr: Arc<Mutex<Option<SocketAddr>>>,
}

impl Services {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        registry: Arc<dyn ClientRegistry>,
        acks: Arc<dyn AckTracker>,
        offline: Arc<dyn OfflineBuffer>,
        cluster: Arc<dyn ClusterPort>,
        router: Arc<dyn Router>,
        forwarder: Arc<dyn Forwarder>,
        shutdown: Arc<Shutdown>,
        config: RexSystemConfig,
        client_shutdowns: Arc<DashMap<u128, CancellationToken>>,
        health: Arc<HealthRegistry>,
        admin_addr: Arc<Mutex<Option<SocketAddr>>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            registry,
            acks,
            offline,
            cluster,
            router,
            forwarder,
            shutdown,
            config,
            client_shutdowns,
            health,
            admin_addr,
        })
    }

    /* ---------------- per-client shutdown tokens ---------------- */

    /// Register a new per-client cancellation token. The caller (transport
    /// loop) receives the token; admin disconnect will cancel it.
    pub fn register_client_shutdown(&self, id: u128) -> CancellationToken {
        let token = CancellationToken::new();
        self.client_shutdowns.insert(id, token.clone());
        token
    }

    /// Remove a per-client token (call when the transport loop has exited).
    pub fn unregister_client_shutdown(&self, id: u128) {
        self.client_shutdowns.remove(&id);
    }

    /// Admin-triggered disconnect: cancel the token if present. Returns
    /// true if a token was found.
    pub fn cancel_client(&self, id: u128) -> bool {
        match self.client_shutdowns.get(&id) {
            Some(t) => {
                t.cancel();
                true
            }
            None => false,
        }
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

    /// ACK setup shared by cast, group, and title handlers. No-op when
    /// ack is disabled. When enabled, generates a msg_id (reusing an
    /// existing one if already set), stamps it into `rex_data`, and
    /// registers the pending ACK. Extracted in C8 from three duplicate
    /// ~14-line blocks.
    pub fn setup_message_ack(
        &self,
        rex_data: &mut rex_core::RexData,
        client_id: u128,
        title: String,
        is_group: bool,
    ) {
        if !self.config.ack_enabled {
            return;
        }
        let msg_id = if rex_data.message_id() != 0 {
            rex_data.message_id()
        } else {
            fastrand::u64(..)
        };
        rex_data.set_message_id(msg_id);
        self.register_pending_ack(msg_id, client_id, title, is_group);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::system::ack::AckTrackerImpl;
    use crate::system::client_registry::ClientRegistryImpl;
    use crate::system::forwarder::NetworkForwarder;
    use crate::system::offline::NoopOfflineBuffer;
    use crate::system::router::ClusterRouter;

    /// Build a `Services` bundle with real implementations / no-op stubs.
    /// Uses the same `TestClusterPort` / `ClientRegistryImpl` /
    /// `AckTrackerImpl` / `NoopOfflineBuffer` / `ClusterRouter` /
    /// `NetworkForwarder` that `handler::test_util::make_services`
    /// uses, but inlines the wiring so this test module does not
    /// depend on the handler test helper itself (which calls
    /// `Services::new`).
    fn make_services() -> Arc<Services> {
        let registry = ClientRegistryImpl::new();
        let cluster: Arc<dyn ClusterPort> =
            Arc::new(crate::handler::test_util::TestClusterPort::new());
        let acks: Arc<dyn AckTracker> = AckTrackerImpl::new(60);
        let offline: Arc<dyn OfflineBuffer> = Arc::new(NoopOfflineBuffer);
        let router: Arc<dyn Router> = ClusterRouter::new(registry.clone(), cluster.clone());
        let forwarder: Arc<dyn Forwarder> = NetworkForwarder::new(
            Arc::new(arc_swap::ArcSwap::from_pointee(None)),
            Arc::new(arc_swap::ArcSwap::from_pointee(None)),
            rex_cluster::types::NodeId::new("test-node"),
            registry.clone(),
        );
        let shutdown = Shutdown::new();
        let config = RexSystemConfig::from_id("test");
        let client_shutdowns: Arc<DashMap<u128, CancellationToken>> = Arc::new(DashMap::new());
        Services::new(
            registry,
            acks,
            offline,
            cluster,
            router,
            forwarder,
            shutdown,
            config,
            client_shutdowns,
            Arc::new(HealthRegistry::new()),
            Arc::new(Mutex::new(None)),
        )
    }

    #[tokio::test]
    async fn cancel_client_propagates() {
        let s = make_services();
        let token = s.register_client_shutdown(0xCAFE);
        assert!(!token.is_cancelled());
        assert!(s.cancel_client(0xCAFE));
        // Yield once so the cancel propagates through any internal channels.
        tokio::task::yield_now().await;
        assert!(token.is_cancelled());
    }

    #[tokio::test]
    async fn cancel_unknown_client_returns_false() {
        let s = make_services();
        assert!(!s.cancel_client(0xDEAD));
    }

    #[tokio::test]
    async fn unregister_removes_token() {
        let s = make_services();
        let _t = s.register_client_shutdown(0xBEEF);
        assert!(s.cancel_client(0xBEEF));
        s.unregister_client_shutdown(0xBEEF);
        assert!(!s.cancel_client(0xBEEF));
    }
}
