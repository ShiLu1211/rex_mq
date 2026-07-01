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
pub use system::{RexSystem, RexSystemConfig, Shutdown};

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use rex_cluster::ClusterConfig as RexClusterConfig;
use rex_core::Protocol;

pub async fn open_server(
    system: Arc<RexSystem>,
    server_config: RexServerConfig,
    shutdown: Arc<Shutdown>,
) -> Result<Arc<dyn RexServerTrait>> {
    // Start cluster manager if enabled
    if let Some(cluster_config) = &server_config.cluster
        && cluster_config.enabled
    {
        start_cluster_manager(&system, cluster_config).await?;
    }

    match server_config.protocol {
        Protocol::Tcp => TcpServer::open(system, server_config, shutdown).await,
        Protocol::Quic => QuicServer::open(system, server_config, shutdown).await,
        Protocol::WebSocket => WebSocketServer::open(system, server_config, shutdown).await,
    }
}

/// Start the cluster manager
async fn start_cluster_manager(system: &Arc<RexSystem>, config: &ClusterConfig) -> Result<()> {
    // Create cluster manager
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

    // Start cluster manager
    cluster_manager.start(cluster_config).await;

    // Set system reference in cluster manager for message delivery
    cluster_manager.set_system(system.clone());

    // Store cluster manager in system
    system.set_cluster_manager(cluster_manager).await;

    info!(
        "Cluster manager started on {} with node_id={}",
        config.cluster_addr, node_id
    );

    Ok(())
}
