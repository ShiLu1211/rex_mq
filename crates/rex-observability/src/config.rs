//! Configuration for the observability framework.

use std::net::SocketAddr;

use crate::tracing_setup::TracingFormat;

#[derive(Clone, Debug)]
pub struct ObservabilityConfig {
    /// Address the admin HTTP server binds to. Default: 127.0.0.1:9090.
    pub admin_addr: SocketAddr,
    /// Token for write/admin endpoints. None = reject all writes.
    pub admin_token: Option<String>,
    /// Tracing output format.
    pub tracing_format: TracingFormat,
    /// Treat a single-node cluster as Healthy in /readyz (skip peer check).
    pub single_node_cluster_ok: bool,
}

impl Default for ObservabilityConfig {
    fn default() -> Self {
        // "127.0.0.1:9090" is a literal that the parser never rejects.
        // Build the `SocketAddr` field-by-field so the workspace lint
        // for `unwrap_used` / `expect_used` / `panic` isn't tripped.
        let admin_addr = SocketAddr::new(
            std::net::IpAddr::V4(std::net::Ipv4Addr::new(127, 0, 0, 1)),
            9090,
        );
        Self {
            admin_addr,
            admin_token: None,
            tracing_format: TracingFormat::Pretty,
            single_node_cluster_ok: true,
        }
    }
}
