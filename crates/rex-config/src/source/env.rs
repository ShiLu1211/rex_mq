//! Env-var override layer: REX__SECTION__KEY.

use crate::error::ConfigError;
use crate::root::RexConfig;

const ENV_PREFIX: &str = "REX__";

pub fn apply_env(mut cfg: RexConfig) -> (RexConfig, Vec<String>) {
    let mut warnings: Vec<String> = Vec::new();
    for (raw_key, raw_val) in std::env::vars() {
        let Some(rest) = raw_key.strip_prefix(ENV_PREFIX) else {
            continue;
        };
        let mut parts = rest.split("__");
        let section = parts.next().unwrap().to_lowercase();
        let key = match parts.next() {
            Some(k) => k.to_lowercase(),
            None => continue,
        };
        if let Err(e) = merge_value(&mut cfg, &section, &key, &raw_val) {
            warnings.push(format!("env REX__{section}__{key}={raw_val:?}: {e}"));
        }
    }
    (cfg, warnings)
}

fn merge_value(
    cfg: &mut RexConfig,
    section: &str,
    key: &str,
    value: &str,
) -> Result<(), ConfigError> {
    let invalid = |reason: &str| -> ConfigError {
        ConfigError::InvalidValue {
            section: section.into(),
            key: key.into(),
            value: value.into(),
            reason: reason.into(),
        }
    };

    match (section, key) {
        ("server", "server_id") => cfg.server.server_id = value.to_string(),
        ("server", "shutdown_grace") => cfg.server.shutdown_grace = parse_u64(section, key, value)?,
        ("server", "check_interval") => cfg.server.check_interval = parse_u64(section, key, value)?,
        ("server", "client_timeout") => cfg.server.client_timeout = parse_u64(section, key, value)?,

        ("cluster", "enabled") => cfg.cluster.enabled = parse_bool(section, key, value)?,
        ("cluster", "cluster_addr") => {
            cfg.cluster.cluster_addr = Some(
                value
                    .parse()
                    .map_err(|_| invalid("must parse as SocketAddr"))?,
            );
        }
        ("cluster", "node_id") => cfg.cluster.node_id = value.to_string(),
        ("cluster", "seed_nodes") => {
            cfg.cluster.seed_nodes = parse_seed_nodes(section, key, value)?
        }

        ("persistence", "enabled") => cfg.persistence.enabled = parse_bool(section, key, value)?,
        ("persistence", "path") => cfg.persistence.path = value.to_string(),
        ("persistence.offline", "enabled") => {
            cfg.persistence.offline.enabled = parse_bool(section, key, value)?
        }
        ("persistence.offline", "ttl_secs") => {
            cfg.persistence.offline.ttl_secs = parse_u64(section, key, value)?
        }
        ("persistence.offline", "ghost_ttl_secs") => {
            cfg.persistence.offline.ghost_ttl_secs = parse_u64(section, key, value)?
        }

        ("ack", "enabled") => cfg.ack.enabled = parse_bool(section, key, value)?,
        ("ack", "timeout_ms") => cfg.ack.timeout_ms = parse_u64(section, key, value)?,
        ("ack", "retries") => cfg.ack.retries = parse_u32(section, key, value)?,

        ("observability", "admin_token") => {
            cfg.observability.admin_token = if value.is_empty() {
                None
            } else {
                Some(value.to_string())
            };
        }
        ("observability", "admin_addr") => {
            cfg.observability.admin_addr = value
                .parse()
                .map_err(|_| invalid("must parse as SocketAddr"))?;
        }
        ("observability", "tracing_format") => {
            cfg.observability.tracing_format = match value {
                "pretty" => rex_observability::tracing_setup::TracingFormat::Pretty,
                "json" => rex_observability::tracing_setup::TracingFormat::Json,
                _ => return Err(invalid("must be 'pretty' or 'json'")),
            };
        }
        ("observability", "single_node_cluster_ok") => {
            cfg.observability.single_node_cluster_ok = parse_bool(section, key, value)?;
        }

        _ => {} // unrecognized — silently ignored (warned by caller)
    }
    Ok(())
}

fn parse_bool(section: &str, key: &str, s: &str) -> Result<bool, ConfigError> {
    match s {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(ConfigError::InvalidValue {
            section: section.into(),
            key: key.into(),
            value: s.into(),
            reason: "must be true/false/1/0/yes/no".into(),
        }),
    }
}

fn parse_u64(section: &str, key: &str, s: &str) -> Result<u64, ConfigError> {
    s.parse().map_err(|_| ConfigError::InvalidValue {
        section: section.into(),
        key: key.into(),
        value: s.into(),
        reason: "must be a non-negative integer".into(),
    })
}

fn parse_u32(section: &str, key: &str, s: &str) -> Result<u32, ConfigError> {
    s.parse().map_err(|_| ConfigError::InvalidValue {
        section: section.into(),
        key: key.into(),
        value: s.into(),
        reason: "must be a non-negative integer".into(),
    })
}

fn parse_seed_nodes(
    section: &str,
    key: &str,
    s: &str,
) -> Result<Vec<std::net::SocketAddr>, ConfigError> {
    s.split(',')
        .map(|x| x.trim())
        .filter(|x| !x.is_empty())
        .map(|x| {
            x.parse().map_err(|_| ConfigError::InvalidValue {
                section: section.into(),
                key: key.into(),
                value: x.into(),
                reason: "must parse as ip:port".into(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use temp_env::with_var;

    #[test]
    fn env_overrides_field() {
        with_var("REX__SERVER__CHECK_INTERVAL", Some("30"), || {
            let cfg = RexConfig::default();
            let (out, _warn) = apply_env(cfg);
            assert_eq!(out.server.check_interval, 30);
        });
    }

    #[test]
    fn no_env_vars_is_noop() {
        let cfg = RexConfig::default();
        let (out, _warn) = apply_env(cfg);
        assert_eq!(out.server.server_id, "rex-server");
    }

    #[test]
    fn non_rex_prefix_is_ignored() {
        with_var("PERSISTENCE_ENABLED", Some("true"), || {
            let cfg = RexConfig::default();
            let (out, _warn) = apply_env(cfg);
            assert!(out.persistence.enabled, "default value unchanged");
        });
    }
}
