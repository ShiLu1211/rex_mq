use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RexSystemConfig {
    #[serde(default = "default_server_id")]
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
fn default_server_id() -> String {
    "rex".to_string()
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
        // Pull each default from its default_*() helper so the value
        // lives in one place (also referenced by the serde derives).
        Self {
            server_id: server_id.to_string(),
            check_interval: default_check_interval(),
            client_timeout: default_client_timeout(),
            persistence_enabled: default_persistence_enabled(),
            persistence_path: default_persistence_path(),
            offline_enabled: default_offline_enabled(),
            offline_ttl: default_offline_ttl(),
            ack_enabled: default_ack_enabled(),
            ack_timeout: default_ack_timeout(),
            ack_retries: default_ack_retries(),
            ghost_ttl_secs: default_ghost_ttl_secs(),
            observability: rex_observability::ObservabilityConfig::default(),
        }
    }
}

impl From<&rex_config::RexConfig> for RexSystemConfig {
    fn from(r: &rex_config::RexConfig) -> Self {
        Self {
            server_id: r.server.server_id.clone(),
            check_interval: r.server.check_interval,
            client_timeout: r.server.client_timeout,
            persistence_enabled: r.persistence.enabled,
            persistence_path: r.persistence.path.clone(),
            offline_enabled: r.persistence.offline.enabled,
            offline_ttl: r.persistence.offline.ttl_secs,
            ack_enabled: r.ack.enabled,
            ack_timeout: r.ack.timeout_ms,
            ack_retries: r.ack.retries,
            ghost_ttl_secs: r.persistence.offline.ghost_ttl_secs,
            observability: r.observability.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_id_matches_serde_defaults() {
        // from_id and the serde defaults should agree on every field.
        // If they drift, a caller using one path gets a different
        // config than a caller using the other path.
        let from_id = RexSystemConfig::from_id("test");
        let toml = "";
        let parsed: RexSystemConfig = toml::from_str(toml).expect("parse empty toml");
        assert_eq!(from_id.check_interval, parsed.check_interval);
        assert_eq!(from_id.client_timeout, parsed.client_timeout);
        assert_eq!(from_id.persistence_enabled, parsed.persistence_enabled);
        assert_eq!(from_id.persistence_path, parsed.persistence_path);
        assert_eq!(from_id.offline_enabled, parsed.offline_enabled);
        assert_eq!(from_id.offline_ttl, parsed.offline_ttl);
        assert_eq!(from_id.ack_enabled, parsed.ack_enabled);
        assert_eq!(from_id.ack_timeout, parsed.ack_timeout);
        assert_eq!(from_id.ack_retries, parsed.ack_retries);
        assert_eq!(from_id.ghost_ttl_secs, parsed.ghost_ttl_secs);
        assert_eq!(
            from_id.ghost_ttl_secs, 86_400,
            "ghost_ttl_secs default is 24h"
        );
    }

    #[test]
    fn ghost_ttl_secs_round_trips_through_toml() {
        // Field-level round-trip: parse a TOML with just ghost_ttl_secs set,
        // re-serialise, and confirm the value survives the round trip.
        let toml_src = "ghost_ttl_secs = 3600\n";
        let parsed: RexSystemConfig = toml::from_str(toml_src).expect("parse toml");
        assert_eq!(parsed.ghost_ttl_secs, 3600);

        let serialised = toml::to_string(&parsed).expect("serialise");
        let reparsed: RexSystemConfig = toml::from_str(&serialised).expect("reparse");
        assert_eq!(reparsed.ghost_ttl_secs, 3600);
    }
}
