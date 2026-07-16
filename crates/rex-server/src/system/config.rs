use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RexSystemConfig {
    pub server_id: String,
    #[serde(default = "default_check_interval")]
    pub check_interval: u64,
    #[serde(default = "default_client_timeout")]
    pub client_timeout: u64,
    // Persistence config
    #[serde(default = "default_persistence_enabled")]
    pub persistence_enabled: bool,
    #[serde(default = "default_persistence_path")]
    pub persistence_path: String,
    #[serde(default = "default_offline_enabled")]
    pub offline_enabled: bool,
    #[serde(default = "default_offline_ttl")]
    pub offline_ttl: u64,
    // ACK config
    #[serde(default = "default_ack_enabled")]
    pub ack_enabled: bool,
    #[serde(default = "default_ack_timeout")]
    pub ack_timeout: u64,
    #[serde(default = "default_ack_retries")]
    pub ack_retries: u32,
    /// TTL (seconds) for restored ghost entries on restart. A live client
    /// saved with `ghost_until = now + ghost_ttl_secs` becomes a ghost with
    /// the same TTL on the next restart. Default 86400s (24h).
    #[serde(default = "default_ghost_ttl_secs")]
    pub ghost_ttl_secs: u64,
    /// Observability stack configuration (admin addr, auth token,
    /// tracing format, single-node tolerance). Defaults to the
    /// `ObservabilityConfig::default()` shape so existing call sites
    /// that don't touch this field keep working unchanged.
    #[serde(default, skip)]
    pub observability: rex_observability::ObservabilityConfig,
}

fn default_check_interval() -> u64 {
    15
}
fn default_client_timeout() -> u64 {
    45
}
fn default_persistence_enabled() -> bool {
    true
}
fn default_persistence_path() -> String {
    "./.rex_sled".to_string()
}
fn default_offline_enabled() -> bool {
    true
}
fn default_offline_ttl() -> u64 {
    86400 * 7 // 7 days
}
fn default_ack_enabled() -> bool {
    false
}
fn default_ack_timeout() -> u64 {
    5000 // 5 seconds
}
fn default_ack_retries() -> u32 {
    3
}
fn default_ghost_ttl_secs() -> u64 {
    86400
}

impl RexSystemConfig {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        server_id: String,
        check_interval: u64,
        client_timeout: u64,
        persistence_enabled: bool,
        persistence_path: String,
        offline_enabled: bool,
        offline_ttl: u64,
        ack_enabled: bool,
        ack_timeout: u64,
        ack_retries: u32,
        ghost_ttl_secs: u64,
    ) -> Self {
        Self {
            server_id,
            check_interval,
            client_timeout,
            persistence_enabled,
            persistence_path,
            offline_enabled,
            offline_ttl,
            ack_enabled,
            ack_timeout,
            ack_retries,
            ghost_ttl_secs,
            observability: rex_observability::ObservabilityConfig::default(),
        }
    }

    pub fn from_id(server_id: &str) -> Self {
        Self {
            server_id: server_id.to_string(),
            check_interval: 15,
            client_timeout: 45,
            persistence_enabled: true,
            persistence_path: "./.rex_sled".to_string(),
            offline_enabled: true,
            offline_ttl: 86400 * 7,
            ack_enabled: false,
            ack_timeout: 5000,
            ack_retries: 3,
            ghost_ttl_secs: 86400,
            observability: rex_observability::ObservabilityConfig::default(),
        }
    }
}
