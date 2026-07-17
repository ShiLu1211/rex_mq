//! Semantic validation pass.

use crate::error::ConfigError;
use crate::root::RexConfig;

pub fn validate(cfg: &RexConfig) -> Result<(), ConfigError> {
    validate_server(cfg)?;
    validate_endpoints(cfg)?;
    validate_cluster(cfg)?;
    validate_persistence(cfg)?;
    validate_ack(cfg)?;
    validate_observability(cfg)?;
    validate_cross_section(cfg)?;
    Ok(())
}

fn validate_server(c: &RexConfig) -> Result<(), ConfigError> {
    if c.server.server_id.is_empty() || c.server.server_id.len() > 64 {
        return Err(ConfigError::InvalidValue {
            section: "server".into(),
            key: "server_id".into(),
            value: c.server.server_id.clone(),
            reason: "must be non-empty and ≤ 64 chars".into(),
        });
    }
    if c.server.shutdown_grace > 600 {
        return Err(ConfigError::InvalidValue {
            section: "server".into(),
            key: "shutdown_grace".into(),
            value: c.server.shutdown_grace.to_string(),
            reason: "must be ≤ 600".into(),
        });
    }
    if c.server.check_interval == 0 || c.server.check_interval > 3_600 {
        return Err(ConfigError::InvalidValue {
            section: "server".into(),
            key: "check_interval".into(),
            value: c.server.check_interval.to_string(),
            reason: "must be in [1, 3600]".into(),
        });
    }
    if c.server.client_timeout == 0 || c.server.client_timeout > 86_400 {
        return Err(ConfigError::InvalidValue {
            section: "server".into(),
            key: "client_timeout".into(),
            value: c.server.client_timeout.to_string(),
            reason: "must be in [1, 86400]".into(),
        });
    }
    if c.server.check_interval > c.server.client_timeout {
        return Err(ConfigError::Semantic(format!(
            "server.check_interval ({}) must be ≤ client_timeout ({})",
            c.server.check_interval, c.server.client_timeout
        )));
    }
    Ok(())
}

fn validate_endpoints(c: &RexConfig) -> Result<(), ConfigError> {
    let enabled_count = c.endpoints.iter().filter(|e| e.enabled).count();
    if enabled_count == 0 {
        return Err(ConfigError::Semantic(
            "at least one [[endpoints]] entry must have enabled = true".into(),
        ));
    }
    for (idx, ep) in c.endpoints.iter().enumerate() {
        if ep.max_buffer_size < 1024 || ep.max_buffer_size > 64 * 1024 * 1024 {
            return Err(ConfigError::InvalidValue {
                section: format!("endpoints[{idx}]"),
                key: "max_buffer_size".into(),
                value: ep.max_buffer_size.to_string(),
                reason: "must be in [1024, 64 MiB]".into(),
            });
        }
        if ep.max_concurrent_handlers == 0 || ep.max_concurrent_handlers > 65_535 {
            return Err(ConfigError::InvalidValue {
                section: format!("endpoints[{idx}]"),
                key: "max_concurrent_handlers".into(),
                value: ep.max_concurrent_handlers.to_string(),
                reason: "must be in [1, 65535]".into(),
            });
        }
    }
    Ok(())
}

fn validate_cluster(c: &RexConfig) -> Result<(), ConfigError> {
    if c.cluster.enabled && c.cluster.cluster_addr.is_none() {
        return Err(ConfigError::Semantic(
            "cluster.enabled = true but cluster.cluster_addr is not set".into(),
        ));
    }
    if c.cluster.seed_nodes.len() > 64 {
        return Err(ConfigError::InvalidValue {
            section: "cluster".into(),
            key: "seed_nodes".into(),
            value: c.cluster.seed_nodes.len().to_string(),
            reason: "must be ≤ 64 entries".into(),
        });
    }
    if c.cluster.node_id.len() > 64 || c.cluster.node_id.contains(' ') {
        return Err(ConfigError::InvalidValue {
            section: "cluster".into(),
            key: "node_id".into(),
            value: c.cluster.node_id.clone(),
            reason: "must be ≤ 64 chars and contain no whitespace".into(),
        });
    }
    Ok(())
}

fn validate_persistence(c: &RexConfig) -> Result<(), ConfigError> {
    if c.persistence.path.is_empty() {
        return Err(ConfigError::MissingRequired {
            section: "persistence".into(),
            key: "path".into(),
        });
    }
    let ttl_range = 60..=(30 * 86_400);
    if !ttl_range.contains(&c.persistence.offline.ttl_secs) {
        return Err(ConfigError::InvalidValue {
            section: "persistence.offline".into(),
            key: "ttl_secs".into(),
            value: c.persistence.offline.ttl_secs.to_string(),
            reason: "must be in [60, 30d]".into(),
        });
    }
    if !ttl_range.contains(&c.persistence.offline.ghost_ttl_secs) {
        return Err(ConfigError::InvalidValue {
            section: "persistence.offline".into(),
            key: "ghost_ttl_secs".into(),
            value: c.persistence.offline.ghost_ttl_secs.to_string(),
            reason: "must be in [60, 30d]".into(),
        });
    }
    Ok(())
}

fn validate_ack(c: &RexConfig) -> Result<(), ConfigError> {
    if c.ack.timeout_ms < 10 || c.ack.timeout_ms > 60_000 {
        return Err(ConfigError::InvalidValue {
            section: "ack".into(),
            key: "timeout_ms".into(),
            value: c.ack.timeout_ms.to_string(),
            reason: "must be in [10, 60000]".into(),
        });
    }
    if c.ack.retries > 16 {
        return Err(ConfigError::InvalidValue {
            section: "ack".into(),
            key: "retries".into(),
            value: c.ack.retries.to_string(),
            reason: "must be ≤ 16".into(),
        });
    }
    Ok(())
}

fn validate_observability(c: &RexConfig) -> Result<(), ConfigError> {
    if c.observability.admin_addr.port() == 0 {
        return Err(ConfigError::InvalidValue {
            section: "observability".into(),
            key: "admin_addr".into(),
            value: c.observability.admin_addr.to_string(),
            reason: "port 0 (any) is not allowed for the admin server".into(),
        });
    }
    Ok(())
}

fn validate_cross_section(c: &RexConfig) -> Result<(), ConfigError> {
    if !c.persistence.enabled && c.persistence.offline.enabled {
        return Err(ConfigError::Semantic(
            "persistence.offline.enabled requires persistence.enabled = true".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_validates() {
        validate(&RexConfig::default()).expect("default must pass");
    }

    #[test]
    fn rejects_check_interval_over_client_timeout() {
        let mut cfg = RexConfig::default();
        cfg.server.check_interval = 100;
        cfg.server.client_timeout = 50;
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Semantic(_)));
    }

    #[test]
    fn rejects_zero_endpoints_enabled() {
        let mut cfg = RexConfig::default();
        cfg.endpoints.iter_mut().for_each(|e| e.enabled = false);
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Semantic(_)));
    }

    #[test]
    fn rejects_cluster_enabled_without_addr() {
        let mut cfg = RexConfig::default();
        cfg.cluster.enabled = true;
        cfg.cluster.cluster_addr = None;
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Semantic(_)));
    }

    #[test]
    fn rejects_offline_without_persistence() {
        let mut cfg = RexConfig::default();
        cfg.persistence.enabled = false;
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::Semantic(_)));
    }

    #[test]
    fn rejects_max_buffer_over_64_mib() {
        let mut cfg = RexConfig::default();
        cfg.endpoints[0].max_buffer_size = 65 * 1024 * 1024;
        let err = validate(&cfg).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidValue { .. }));
    }
}
