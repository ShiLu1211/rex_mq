mod aggregate;
mod cluster;
mod config;
pub mod handler;
mod server;
mod system;
mod transport;

pub use crate::cluster::server_cluster::ServerClusterManager;
pub use crate::cluster::{ClusterIntegration, ForwardRequest, ForwardType};
pub use crate::config::{ClusterConfig, RexServerConfig};
pub use crate::transport::{QuicServer, TcpServer, WebSocketServer};
pub use aggregate::*;
pub use server::RexServerTrait;
pub use system::{
    AckTracker, AckTrackerImpl, ClientRegistry, ClientRegistryImpl, ClusterPort, Janitor,
    PendingAckInfo, RexSystem, RexSystemConfig, Services, Shutdown,
};

use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use tracing::info;

use rex_cluster::ClusterConfig as RexClusterConfig;
use rex_cluster::types::ClusterMessage;
use rex_core::Protocol;

pub async fn open_server(
    system: Arc<RexSystem>,
    server_config: RexServerConfig,
) -> Result<Arc<dyn RexServerTrait>> {
    // Build the Services bundle from the system's ports. The cluster port
    // comes from RexSystem's cluster_manager when present, otherwise a
    // noop stand-in. Commit 8 will route cluster bootstrap through Services
    // directly; for now we keep the existing RexSystem-based path.
    let services = build_services_from_system(&system);

    // Start cluster manager if enabled. This path goes through RexSystem
    // for commit 7; commit 8 will route it through Services directly.
    if let Some(cluster_config) = &server_config.cluster
        && cluster_config.enabled
    {
        start_cluster_manager(&system, cluster_config).await?;
    }

    // Spawn the Janitor for periodic cleanup. One task per server.
    let check_interval = Duration::from_secs(system.config.check_interval);
    let client_timeout = system.config.client_timeout;
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

/// Build a `Services` bundle from an existing `RexSystem` — extracts the
/// ports via their accessors and packs them into a new struct. Used by
/// `open_server` for now; commit 8 deletes `RexSystem` and constructs
/// `Services` directly.
fn build_services_from_system(system: &Arc<RexSystem>) -> Arc<Services> {
    let cluster: Arc<dyn ClusterPort> = system
        .cluster_manager()
        .map(|c| c as Arc<dyn ClusterPort>)
        .unwrap_or_else(|| Arc::new(NoopClusterPort) as Arc<dyn ClusterPort>);
    Arc::new(Services {
        registry: system.registry(),
        acks: system.acks(),
        offline: system.offline(),
        cluster,
        shutdown: system.shutdown.clone(),
        config: system.config.clone(),
    })
}

/// Start the cluster manager and wire it into the system.
async fn start_cluster_manager(system: &Arc<RexSystem>, config: &ClusterConfig) -> Result<()> {
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
    cluster_manager.set_system(system.clone());
    system.set_cluster_manager(cluster_manager).await;

    info!(
        "Cluster manager started on {} with node_id={}",
        config.cluster_addr, node_id
    );

    Ok(())
}

/// Noop cluster port — used when no cluster is configured. Behaves as if
/// the local node owned everything.
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
