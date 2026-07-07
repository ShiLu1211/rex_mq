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
    ClientSnapshot, ClusterPort, ClusterRouter, DeliveryOutcome, Forwarder, FwdResult, Janitor,
    NetworkForwarder, NoopOfflineBuffer, OfflineBuffer, PendingAckInfo, RegistryObsAdapter,
    RexSystemConfig, RoutePlan, Router, Services, Shutdown, SledOfflineBuffer,
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
    )?;
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

/// Build a `Services` bundle. This is the canonical construction site.
/// Takes an optional cluster port — when `None`, a `NoopClusterPort` is
/// used (behaves as if the local node owns everything).
pub async fn build_services(
    config: RexSystemConfig,
    shutdown: Arc<Shutdown>,
    cluster: Option<Arc<dyn ClusterPort>>,
) -> Arc<Services> {
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

    Services::new(
        registry,
        acks,
        offline,
        cluster,
        router,
        forwarder,
        shutdown,
        config,
        Arc::new(dashmap::DashMap::new()),
        Arc::new(rex_observability::health::HealthRegistry::new()),
        Arc::new(parking_lot::Mutex::new(None::<SocketAddr>)),
    )
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
