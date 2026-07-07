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

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use arc_swap::ArcSwap;
use tracing::{info, warn};

use rex_cluster::ClusterConfig as RexClusterConfig;
use rex_cluster::types::ClusterMessage;
use rex_core::Protocol;

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
