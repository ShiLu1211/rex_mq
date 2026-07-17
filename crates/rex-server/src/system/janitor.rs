//! Periodic cleanup task.
//!
//! Owns no state of its own — borrows `Arc<Services>` to reach the
//! `ClientRegistry` and `AckTracker` ports. Effects (close inactive clients,
//! deliver ACK timeouts) live here so the ports stay pure state.

use std::{sync::Arc, time::Duration};

use rex_core::{RetCode, RexCommand, utils::now_secs};
use tracing::{info, warn};

use crate::Services;

/// Background cleanup driver. Spawned by `lib.rs::open_server`.
pub struct Janitor {
    services: Arc<Services>,
}

impl Janitor {
    pub fn new(services: Arc<Services>) -> Self {
        Self { services }
    }

    /// Periodic loop. Wakes every `check_interval` seconds and runs both
    /// cleanup arms. Exits when `Services::shutdown` signals.
    pub async fn run(self, check_interval: Duration, client_timeout: u64) {
        let mut shutdown_rx = self.services.shutdown.subscribe();

        loop {
            tokio::select! {
                _ = tokio::time::sleep(check_interval) => {
                    self.cleanup_inactive_clients(client_timeout).await;
                    self.cleanup_expired_acks().await;
                    self.cleanup_expired_ghosts(now_secs()).await;
                }
                _ = shutdown_rx.recv() => {
                    info!("Janitor received shutdown signal, stopping.");
                    break;
                }
            }
        }
    }

    /// For each ghost whose `ghost_until` has passed, drop the registry
    /// entry, unregister from the cluster, and clear its offline messages.
    /// Best-effort: a failure on any step is logged at warn; the loop
    /// continues to the next id.
    pub async fn cleanup_expired_ghosts(&self, now: u64) {
        let expired = self.services.state_store.take_expired_ghosts(now).await;
        for client_id in expired {
            self.services.registry.remove_ghost(client_id);
            self.services.cluster.unregister_client(client_id);
            self.services
                .offline
                .clear_offline_messages(client_id)
                .await;
            tracing::info!("Ghost for client {:032X} expired, removed", client_id);
        }
    }

    /// Find clients whose last_recv is older than `client_timeout` and close
    /// them. Uses `registry.take_inactive` to identify candidates and
    /// `registry.remove_client` to take ownership of the `Arc<RexClientInner>`
    /// for closing.
    async fn cleanup_inactive_clients(&self, client_timeout: u64) {
        let stale_ids = self.services.registry.take_inactive(client_timeout);
        for client_id in stale_ids {
            let Some(client) = self.services.registry.remove_client(client_id) else {
                continue;
            };

            warn!(
                "Client [{:032X}] (addr: {}) timed out, removing...",
                client_id,
                client.local_addr()
            );

            if let Err(e) = client.close().await {
                warn!("close client [{:032X}] error: {}", client_id, e);
            } else {
                info!("client [{:032X}] removed", client_id);
            }
        }
    }

    /// For each ACK the tracker reports as expired, look up the original
    /// sender via the registry and deliver an `AckReturn` with
    /// `RetCode::AckTimeout`.
    async fn cleanup_expired_acks(&self) {
        if !self.services.is_ack_enabled() {
            return;
        }
        let now = now_secs();

        for (msg_id, source_client_id) in self.services.take_expired_acks(now) {
            let Some(sender) = self.services.registry.find_some_by_id(source_client_id) else {
                continue;
            };

            let ack_data = rex_core::AckData::new(msg_id);
            let rex_data = ack_data.to_rex_data(source_client_id, RexCommand::AckReturn);
            let mut rex_data = rex_data;
            rex_data.set_retcode(RetCode::AckTimeout);

            if let Err(e) = sender.send_buf(rex_data.pack_ref()).await {
                warn!("Failed to send ACK timeout to client: {}", e);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::TestClusterPort;
    use crate::system::ack::AckTrackerImpl;
    use crate::system::client_registry::ClientRegistryImpl;
    use crate::system::client_state_store::{ClientStateStore, SledClientStateStore};
    use crate::system::forwarder::NetworkForwarder;
    use crate::system::offline::{OfflineBuffer, SledOfflineBuffer};
    use crate::system::router::ClusterRouter;
    use dashmap::DashMap;
    use parking_lot::Mutex;
    use rex_observability::health::HealthRegistry;
    use std::sync::atomic::{AtomicU64, Ordering};
    use tokio_util::sync::CancellationToken;

    static SLED_COUNTER: AtomicU64 = AtomicU64::new(0);
    fn fresh_sled_path() -> String {
        let n = SLED_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        std::env::temp_dir()
            .join(format!("rex-janitor-test-{pid}-{n}"))
            .to_string_lossy()
            .to_string()
    }

    /// Build a Services bundle with a real SledClientStateStore + a real
    /// SledOfflineBuffer (sharing one sled::Db) so the Janitor's cleanup
    /// paths can actually persist and read back. Returns the typed
    /// `TestClusterPort` handle so the test can assert unregister calls.
    async fn make_services_with_shared_sled() -> (Arc<crate::Services>, Arc<TestClusterPort>, String)
    {
        let path = fresh_sled_path();
        std::fs::create_dir_all(&path).unwrap();
        let db = sled::open(&path).unwrap();
        let db = Arc::new(db);
        let offline: Arc<dyn OfflineBuffer> = SledOfflineBuffer::with_db(db.clone());
        let state_store: Arc<dyn ClientStateStore> = SledClientStateStore::with_db(db.clone());

        let registry = ClientRegistryImpl::new();
        let test_cluster = Arc::new(TestClusterPort::new());
        let cluster: Arc<dyn crate::system::cluster_port::ClusterPort> = test_cluster.clone();
        let acks: Arc<dyn crate::system::ack::AckTracker> = AckTrackerImpl::new(60);
        let router: Arc<dyn crate::system::router::Router> =
            ClusterRouter::new(registry.clone(), cluster.clone());
        let forwarder: Arc<dyn crate::system::forwarder::Forwarder> = NetworkForwarder::new(
            Arc::new(arc_swap::ArcSwap::from_pointee(None)),
            Arc::new(arc_swap::ArcSwap::from_pointee(None)),
            rex_cluster::types::NodeId::new("test-node"),
            registry.clone(),
        );
        let shutdown = crate::Shutdown::new();
        let config = crate::RexSystemConfig::from_id("test");
        let client_shutdowns: Arc<DashMap<u128, CancellationToken>> = Arc::new(DashMap::new());
        let s = crate::Services::new(
            registry,
            acks,
            offline,
            cluster,
            router,
            forwarder,
            state_store,
            shutdown,
            config,
            client_shutdowns,
            Arc::new(HealthRegistry::new()),
            Arc::new(Mutex::new(None)),
        );
        (s, test_cluster, path)
    }

    #[tokio::test]
    async fn cleanup_expired_ghosts_removes_expired_and_clears_messages() {
        let (s, test_cluster, path) = make_services_with_shared_sled().await;

        // Seed three persisted ghost rows with differing expiries.
        s.state_store.save(1, &[], 0, 0).await; // expired
        s.state_store.save(2, &[], 0, u64::MAX).await; // never
        s.state_store
            .save(3, &[], 0, rex_core::utils::now_secs() + 3600)
            .await; // fresh

        // Mirror the ghost rows in the registry (so remove_ghost has work).
        s.registry.add_ghost(1, vec![], 0).unwrap();
        s.registry.add_ghost(2, vec![], u64::MAX).unwrap();
        s.registry
            .add_ghost(3, vec![], rex_core::utils::now_secs() + 3600)
            .unwrap();
        s.cluster.register_client(1);
        s.cluster.register_client(2);
        s.cluster.register_client(3);

        // Queue a message for the expired ghost so we can assert it gets cleared.
        s.offline
            .queue_offline_message(1, "x", bytes::Bytes::from_static(b"hello"))
            .await;

        let janitor = Janitor::new(s.clone());
        janitor
            .cleanup_expired_ghosts(rex_core::utils::now_secs())
            .await;

        // Expired ghost gone; fresh ones still present.
        assert_eq!(s.registry.ghost_count(), 2);
        assert!(s.registry.ghost_titles(1).is_none());
        // Messages for ghost 1 cleared
        assert_eq!(s.offline.get_offline_count(1).await, 0);
        // Cluster unregistered
        let unreg = test_cluster.unregister_calls_test();
        assert!(unreg.contains(&1));

        let _ = std::fs::remove_dir_all(&path);
    }
}
