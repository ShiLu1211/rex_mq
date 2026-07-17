//! CLI flag overrides — populated by `rex-cli`.

use crate::root::RexConfig;

#[derive(Debug, Clone, Default)]
pub struct CliOverrides {
    pub config_path: Option<std::path::PathBuf>,
    pub server_id: Option<String>,
    pub persist: Option<bool>,
    pub cluster_enabled: bool,
    pub cluster_addr: Option<String>,
    pub seeds: Option<String>,
    pub admin_write: bool,
}

pub fn apply_cli(mut cfg: RexConfig, cli: &CliOverrides) -> (RexConfig, Vec<String>) {
    let mut warnings: Vec<String> = Vec::new();

    if let Some(v) = &cli.server_id {
        warnings.push("--server-id is deprecated; set [server].server_id in rex.toml".into());
        cfg.server.server_id = v.clone();
    }
    if let Some(v) = cli.persist {
        cfg.persistence.enabled = v;
    }
    if cli.cluster_enabled {
        if let Some(ref addr_str) = cli.cluster_addr {
            if let Ok(addr) = addr_str.parse::<std::net::SocketAddr>() {
                cfg.cluster.cluster_addr = Some(addr);
            } else {
                warnings.push(format!("invalid --cluster-addr: {addr_str} (ignored)"));
            }
        } else if cfg.cluster.cluster_addr.is_none() {
            cfg.cluster.cluster_addr = "0.0.0.0:19882".parse().ok();
        }
        cfg.cluster.enabled = true;
    }
    if let Some(seeds) = &cli.seeds {
        let parsed: Vec<std::net::SocketAddr> = seeds
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse() {
                Ok(a) => Some(a),
                Err(_) => {
                    warnings.push(format!("invalid seed address: {s} (skipped)"));
                    None
                }
            })
            .collect();
        cfg.cluster.seed_nodes = parsed;
    }
    (cfg, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cli_persist_overrides_toml_value() {
        let mut cfg = RexConfig::default();
        cfg.persistence.enabled = false;
        let cli = CliOverrides {
            persist: Some(true),
            ..Default::default()
        };
        let (out, w) = apply_cli(cfg, &cli);
        assert!(out.persistence.enabled);
        assert!(w.is_empty());
    }

    #[test]
    fn cli_cluster_flag_with_no_toml_cluster_enables_it() {
        let cfg = RexConfig::default();
        let cli = CliOverrides {
            cluster_enabled: true,
            cluster_addr: Some("127.0.0.1:19882".into()),
            ..Default::default()
        };
        let (out, _) = apply_cli(cfg, &cli);
        assert!(out.cluster.enabled);
        assert_eq!(out.cluster.cluster_addr.unwrap().port(), 19882);
    }

    #[test]
    fn cli_seeds_comma_list_parses() {
        let cfg = RexConfig::default();
        let cli = CliOverrides {
            cluster_enabled: true,
            cluster_addr: Some("127.0.0.1:19882".into()),
            seeds: Some("10.0.0.2:19882, 10.0.0.3:19882".into()),
            ..Default::default()
        };
        let (out, _) = apply_cli(cfg, &cli);
        assert_eq!(out.cluster.seed_nodes.len(), 2);
    }
}
