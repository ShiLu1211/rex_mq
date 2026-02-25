use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::Protocol;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RexServerConfig {
    pub protocol: Protocol,
    pub bind_addr: SocketAddr,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_buffer")]
    pub max_buffer_size: usize,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_handlers: usize,
    /// Cluster mode configuration
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cluster: Option<ClusterConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterConfig {
    /// Enable cluster mode
    pub enabled: bool,
    /// Cluster listen address (for node-to-node communication)
    pub cluster_addr: SocketAddr,
    /// Seed nodes for cluster discovery
    pub seed_nodes: Vec<SocketAddr>,
    /// Node ID (auto-generated if not set)
    pub node_id: Option<String>,
}

fn default_enabled() -> bool {
    true
}
fn default_max_buffer() -> usize {
    8 * 1024 * 1024
}
fn default_max_concurrent() -> usize {
    1000
}

impl RexServerConfig {
    pub fn new(protocol: Protocol, bind_addr: SocketAddr) -> Self {
        Self {
            protocol,
            bind_addr,
            enabled: true,
            max_buffer_size: 8 * 1024 * 1024,
            max_concurrent_handlers: 1000,
            cluster: None,
        }
    }

    pub fn from_addr(bind_addr: SocketAddr) -> Self {
        Self::new(Protocol::Tcp, bind_addr)
    }

    /// Enable cluster mode
    pub fn enable_cluster(mut self, cluster_addr: SocketAddr) -> Self {
        self.cluster = Some(ClusterConfig {
            enabled: true,
            cluster_addr,
            seed_nodes: Vec::new(),
            node_id: None,
        });
        self
    }

    /// Add a seed node for cluster discovery
    pub fn add_seed_node(mut self, addr: SocketAddr) -> Self {
        if let Some(ref mut cluster) = self.cluster {
            cluster.seed_nodes.push(addr);
        }
        self
    }

    /// Set node ID for this cluster node
    pub fn set_node_id(mut self, node_id: String) -> Self {
        if let Some(ref mut cluster) = self.cluster {
            cluster.node_id = Some(node_id);
        }
        self
    }

    /// Check if cluster mode is enabled
    pub fn is_cluster_enabled(&self) -> bool {
        self.cluster.as_ref().map(|c| c.enabled).unwrap_or(false)
    }

    /// Get cluster config
    pub fn cluster_config(&self) -> Option<&ClusterConfig> {
        self.cluster.as_ref()
    }
}
