//! RexConfig schema.

use std::net::SocketAddr;

use rex_core::Protocol;
use serde::{Deserialize, Serialize};

pub use rex_observability::ObservabilityConfig;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RexConfig {
    pub server: ServerSection,
    pub endpoints: Vec<EndpointConfig>,
    pub cluster: ClusterSection,
    pub persistence: PersistenceSection,
    pub ack: AckSection,
    pub observability: ObservabilityConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerSection {
    pub server_id: String,
    #[serde(default = "default_shutdown_grace")]
    pub shutdown_grace: u64,
    #[serde(default = "default_check_interval")]
    pub check_interval: u64,
    #[serde(default = "default_client_timeout")]
    pub client_timeout: u64,
}

fn default_shutdown_grace() -> u64 {
    10
}
fn default_check_interval() -> u64 {
    15
}
fn default_client_timeout() -> u64 {
    45
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EndpointConfig {
    pub protocol: Protocol,
    pub address: SocketAddr,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_buffer_size")]
    pub max_buffer_size: usize,
    #[serde(default = "default_max_concurrent_handlers")]
    pub max_concurrent_handlers: usize,
}

fn default_enabled() -> bool {
    true
}
fn default_max_buffer_size() -> usize {
    8 * 1024 * 1024
}
fn default_max_concurrent_handlers() -> usize {
    1000
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClusterSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub cluster_addr: Option<SocketAddr>,
    #[serde(default = "default_node_id")]
    pub node_id: String,
    #[serde(default)]
    pub seed_nodes: Vec<SocketAddr>,
}

fn default_node_id() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistenceSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_persistence_path")]
    pub path: String,
    #[serde(default)]
    pub offline: OfflineSection,
}

fn default_true() -> bool {
    true
}
fn default_persistence_path() -> String {
    "./.rex_sled".to_string()
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OfflineSection {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_offline_ttl")]
    pub ttl_secs: u64,
    #[serde(default = "default_ghost_ttl")]
    pub ghost_ttl_secs: u64,
}

fn default_offline_ttl() -> u64 {
    7 * 86_400
}
fn default_ghost_ttl() -> u64 {
    86_400
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AckSection {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ack_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_ack_retries")]
    pub retries: u32,
}

fn default_ack_timeout() -> u64 {
    5_000
}
fn default_ack_retries() -> u32 {
    3
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_toml_with_one_endpoint() {
        let toml = r#"
            [server]
            server_id = "rex-server"

            [[endpoints]]
            protocol = "tcp"
            address = "0.0.0.0:8881"
        "#;
        let cfg: RexConfig = toml::from_str(toml).expect("parse");
        assert_eq!(cfg.server.server_id, "rex-server");
        assert_eq!(cfg.endpoints.len(), 1);
        assert_eq!(
            cfg.endpoints[0].address,
            "0.0.0.0:8881".parse::<SocketAddr>().unwrap()
        );
        assert_eq!(cfg.endpoints[0].protocol, Protocol::Tcp);
    }

    #[test]
    fn unknown_key_is_rejected() {
        let toml = r#"
            [server]
            server_id = "rex"
            bogus_field = 1
        "#;
        let err = toml::from_str::<RexConfig>(toml).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("bogus_field") || msg.contains("unknown field"),
            "expected unknown-field error, got: {msg}"
        );
    }
}
