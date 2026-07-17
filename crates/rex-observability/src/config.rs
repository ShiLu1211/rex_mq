//! Configuration for the observability framework.

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::tracing_setup::TracingFormat;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ObservabilityConfig {
    #[serde(default = "default_admin_addr")]
    pub admin_addr: SocketAddr,
    #[serde(default)]
    pub admin_token: Option<String>,
    #[serde(default = "default_tracing_format")]
    pub tracing_format: TracingFormat,
    #[serde(default = "default_single_node_cluster_ok")]
    pub single_node_cluster_ok: bool,
    #[serde(default)]
    pub admin_metrics_token: Option<String>,
}

fn default_admin_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
        9_090,
    )
}
fn default_tracing_format() -> TracingFormat {
    TracingFormat::Pretty
}
fn default_single_node_cluster_ok() -> bool {
    true
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        Self {
            admin_addr: default_admin_addr(),
            admin_token: None,
            tracing_format: default_tracing_format(),
            single_node_cluster_ok: default_single_node_cluster_ok(),
            admin_metrics_token: None,
        }
    }
}
