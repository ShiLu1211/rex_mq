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

impl RexSystemConfig {
    pub fn new(
        server_id: String,
        check_interval: u64,
        client_timeout: u64,
        persistence_enabled: bool,
        persistence_path: String,
        offline_enabled: bool,
        offline_ttl: u64,
    ) -> Self {
        Self {
            server_id,
            check_interval,
            client_timeout,
            persistence_enabled,
            persistence_path,
            offline_enabled,
            offline_ttl,
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
        }
    }
}
