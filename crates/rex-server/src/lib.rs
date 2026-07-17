mod cluster;
mod config;
pub mod handler;
mod server;
mod system;
mod transport;

pub use crate::cluster::{ForwardRequest, ForwardType, ServerClusterManager};
pub use crate::config::{ClusterConfig, RexServerConfig};
pub use crate::transport::{QuicServer, TcpServer, WebSocketServer};
pub use server::RexServerTrait;
pub use system::{
    AckTracker, AckTrackerImpl, ClientCancelAdapter, ClientRegistry, ClientRegistryImpl,
    ClientSnapshot, ClientStateStore, ClusterPort, ClusterRouter, DeliveryOutcome, Forwarder,
    FwdResult, Janitor, NetworkForwarder, NoopClientStateStore, NoopOfflineBuffer, OfflineBuffer,
    PendingAckInfo, RegistryObsAdapter, RexSystemConfig, RoutePlan, Router, Services, Shutdown,
    SledClientStateStore, SledOfflineBuffer,
};

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use tracing::{info, warn};

use rex_cluster::ClusterConfig as RexClusterConfig;
use rex_cluster::types::ClusterMessage;
use rex_core::Protocol;
use rex_observability::metrics::set_cluster_peers;
use rex_observability::probe::traits::{ClusterSnapshot, ForwarderSnapshot, PersistenceSnapshot};
use rex_observability::probe::{
    ClusterHealthProbe, ForwarderHealthProbe, PersistenceHealthProbe, RegistryHealthProbe,
};

pub async fn open_server(
    services: Arc<Services>,
    server_config: RexServerConfig,
) -> Result<Arc<dyn RexServerTrait>> {
    // Start cluster manager if enabled.
    if let Some(cluster_config) = &server_config.cluster
        && cluster_config.enabled
    {
        start_cluster_manager(&services, cluster_config).await?;
    }

    // Restore persisted client state as ghost entries. Best-effort:
    // a load_all failure is logged and the server still starts.
    if services.config.persistence_enabled {
        let restored = services.state_store.load_all().await;
        tracing::info!(
            "Restoring {} client entries from persistence",
            restored.len()
        );
        for entry in restored {
            if let Err(e) = services.registry.add_ghost(
                entry.client_id,
                entry.titles.clone(),
                entry.ghost_until,
            ) {
                tracing::warn!(
                    "Failed to restore ghost for client {:032X}: {:?}",
                    entry.client_id,
                    e
                );
                rex_observability::metrics::inc_client_state_restore("skip");
                continue;
            }
            services.cluster.register_client(entry.client_id);
            rex_observability::metrics::inc_client_state_restore("ok");
            tracing::debug!(
                "Restored ghost for client {:032X} ({} titles, ttl={})",
                entry.client_id,
                entry.titles.len(),
                entry.ghost_until
            );
        }
        rex_observability::metrics::set_client_state_ghosts_current(
            services.registry.ghost_count() as i64,
        );
    }

    // Spawn the Janitor for periodic cleanup.
    let check_interval = Duration::from_secs(services.config.check_interval);
    let client_timeout = services.config.client_timeout;
    let janitor = Janitor::new(services.clone());
    tokio::spawn(async move {
        janitor.run(check_interval, client_timeout).await;
    });

    // Start observability (admin HTTP + tracing) and register the four
    // production probes into the shared HealthRegistry. Built from the
    // narrow observability-side traits so rex-observability stays
    // free of rex-server dependencies. Probes are registered after
    // `start` so they can run against the *live* Services fields
    // (registry, cluster, offline buffer, forwarder).
    let obs_cfg = services.config.observability.clone();
    let registry_adapter: Arc<dyn rex_observability::probe::traits::RegistrySnapshot> =
        Arc::new(RegistryObsAdapter(services.registry.clone()));
    let cancel_adapter: Arc<dyn rex_observability::probe::traits::ClientCancel> =
        Arc::new(ClientCancelAdapter(services.clone()));
    let obs = rex_observability::ObservabilityHandle::start(
        &obs_cfg,
        services.health.clone(),
        Some(registry_adapter.clone()),
        Some(cancel_adapter.clone()),
    )
    .await?;
    // Expose the resolved observability address so tests (and tooling)
    // can scrape `/metrics` even when the config supplied port `0`.
    *services.admin_addr.lock() = Some(obs.http.addr);

    struct ClusterAdapter(Arc<dyn ClusterPort>);
    impl ClusterSnapshot for ClusterAdapter {
        fn peer_count(&self) -> usize {
            self.0.get_nodes().len().saturating_sub(1)
        }
        fn local_node_present(&self) -> bool {
            self.0.get_local_node_id().is_some()
        }
    }
    struct PersistenceAdapter(Arc<dyn OfflineBuffer>);
    impl PersistenceSnapshot for PersistenceAdapter {
        fn last_error(&self) -> Option<String> {
            self.0.last_error()
        }
    }
    struct ClientStateStoreAdapter(Arc<dyn crate::ClientStateStore>);
    impl PersistenceSnapshot for ClientStateStoreAdapter {
        fn last_error(&self) -> Option<String> {
            self.0.last_error()
        }
    }
    struct ForwarderAdapter(Arc<dyn Forwarder>);
    impl ForwarderSnapshot for ForwarderAdapter {
        fn node_manager_ready(&self) -> bool {
            self.0.is_cluster_started()
        }
    }

    obs.health
        .register(Arc::new(RegistryHealthProbe::new(registry_adapter)));
    obs.health.register(Arc::new(ClusterHealthProbe::new(
        Arc::new(ClusterAdapter(services.cluster.clone())),
        obs_cfg.single_node_cluster_ok,
    )));
    obs.health
        .register(Arc::new(PersistenceHealthProbe::new(Arc::new(
            PersistenceAdapter(services.offline.clone()),
        ))));
    obs.health
        .register(Arc::new(PersistenceHealthProbe::new(Arc::new(
            ClientStateStoreAdapter(services.state_store.clone()),
        ))));
    obs.health
        .register(Arc::new(ForwarderHealthProbe::new(Arc::new(
            ForwarderAdapter(services.forwarder.clone()),
        ))));

    // The handle is intentionally leaked into the admin HTTP task —
    // it lives as long as the server. When `open_server` returns,
    // the handle is moved into a tokio task that drops it on shutdown.
    let mut shutdown_rx = services.shutdown.subscribe();
    tokio::spawn(async move {
        // Hold until the shutdown signal fires; on drop the HTTP
        // server's shutdown_tx is sent.
        let _ = shutdown_rx.recv().await;
        obs.shutdown();
    });

    match server_config.protocol {
        Protocol::Tcp => TcpServer::open(services, server_config).await,
        Protocol::Quic => QuicServer::open(services, server_config).await,
        Protocol::WebSocket => WebSocketServer::open(services, server_config).await,
    }
}

/// Open the shared sled `Db` used by both the offline buffer and the
/// client-state store. sled takes an exclusive file lock per path, so
/// opening the same path twice in the same process fails; we open it
/// once here and hand clones of the `Arc` to each adapter.
async fn open_shared_sled_db(path: &str) -> anyhow::Result<Arc<sled::Db>> {
    use anyhow::Context;
    std::fs::create_dir_all(path).context("create persistence dir")?;
    let db = sled::open(path).context("open sled")?;
    Ok(Arc::new(db))
}

/// Build a `Services` bundle. This is the canonical construction site.
/// Takes an optional cluster port — when `None`, a `NoopClusterPort` is
/// used (behaves as if the local node owns everything).
pub async fn build_services(
    config: RexSystemConfig,
    shutdown: Arc<Shutdown>,
    cluster: Option<Arc<dyn ClusterPort>>,
) -> Arc<Services> {
    // Open sled once and share it across the two persistence adapters
    // so they don't contend on sled's exclusive file lock.
    let shared_db: Option<Arc<sled::Db>> = if config.persistence_enabled {
        match open_shared_sled_db(&config.persistence_path).await {
            Ok(db) => Some(db),
            Err(e) => {
                warn!(
                    "Failed to open persistence store at {}: {}, continuing without persistence",
                    config.persistence_path, e
                );
                None
            }
        }
    } else {
        None
    };

    let offline: Arc<dyn OfflineBuffer> = match &shared_db {
        Some(db) => SledOfflineBuffer::with_db(db.clone()),
        None => Arc::new(NoopOfflineBuffer),
    };

    let state_store: Arc<dyn ClientStateStore> = match &shared_db {
        Some(db) => SledClientStateStore::with_db(db.clone()),
        None => Arc::new(NoopClientStateStore),
    };

    let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
    let acks: Arc<dyn AckTracker> = AckTrackerImpl::new(config.ack_timeout);
    let cluster: Arc<dyn ClusterPort> =
        cluster.unwrap_or_else(|| Arc::new(NoopClusterPort) as Arc<dyn ClusterPort>);
    let router: Arc<dyn Router> = ClusterRouter::new(registry.clone(), cluster.clone());

    // Forwarder: construct with empty `node_manager` / `route_table`
    // slots. When the cluster starts (`start_cluster_manager`), the
    // slots get populated. Forwarding before that returns
    // `PeerUnreachable("cluster-not-started")`.
    let local_node_id = cluster
        .get_local_node_id()
        .unwrap_or_else(|| "local".to_string());
    let forwarder: Arc<dyn Forwarder> = NetworkForwarder::new(
        Arc::new(ArcSwap::from_pointee(None)),
        Arc::new(ArcSwap::from_pointee(None)),
        rex_cluster::types::NodeId::new(local_node_id),
        registry.clone(),
    );

    let services = Services::new(
        registry,
        acks,
        offline,
        cluster,
        router,
        forwarder,
        state_store,
        shutdown,
        config,
        Arc::new(dashmap::DashMap::new()),
        Arc::new(rex_observability::health::HealthRegistry::new()),
        Arc::new(parking_lot::Mutex::new(None::<SocketAddr>)),
    );

    // Touch the gauge so it appears in /metrics even when cluster is
    // disabled — the live value is updated by ServerClusterManager as
    // nodes join / leave.
    set_cluster_peers(0);
    services
}

/// Start the cluster manager and wire it into Services.
async fn start_cluster_manager(services: &Arc<Services>, config: &ClusterConfig) -> Result<()> {
    let node_id = config
        .node_id
        .clone()
        .unwrap_or_else(|| "rex-node".to_string());
    let cluster_manager = ServerClusterManager::new(node_id.clone(), true);

    let cluster_config = RexClusterConfig {
        enabled: true,
        node_id: rex_cluster::NodeId::new(node_id.clone()),
        listen_addr: config.cluster_addr,
        seed_nodes: config.seed_nodes.clone(),
        communication_timeout_ms: 1000,
        heartbeat_interval_ms: 1000,
        election_timeout_min_ms: 5000,
        election_timeout_max_ms: 10000,
        max_retries: 3,
    };

    cluster_manager.start(cluster_config).await;
    cluster_manager.set_services(services.clone());

    info!(
        "Cluster manager started on {} with node_id={}",
        config.cluster_addr, node_id
    );

    Ok(())
}

/// Noop cluster port — used when no cluster is configured.
struct NoopClusterPort;

#[async_trait::async_trait]
impl ClusterPort for NoopClusterPort {
    fn register_client(&self, _client_id: u128) {}
    fn unregister_client(&self, _client_id: u128) {}
    fn find_node_for_title(&self, _title: &str) -> Option<String> {
        Some("local".to_string())
    }
    fn get_local_node_id(&self) -> Option<String> {
        Some("local".to_string())
    }
    fn get_nodes(&self) -> Vec<String> {
        vec!["local".to_string()]
    }
    async fn forward_message(&self, _target_node: &str, _request: ForwardRequest) -> bool {
        false
    }
    async fn broadcast(&self, _message: ClusterMessage) -> usize {
        0
    }
}
