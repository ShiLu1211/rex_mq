//! TOML parse step.

use std::path::Path;

use crate::error::ConfigError;
use crate::root::RexConfig as RootConfig;

pub fn parse_toml(path: &Path) -> Result<(RootConfig, Vec<String>), ConfigError> {
    if !path.is_file() {
        return Err(ConfigError::FileNotFound {
            path: path.to_path_buf(),
        });
    }
    let body = std::fs::read_to_string(path).map_err(|e| ConfigError::Io {
        path: path.to_path_buf(),
        source: e,
    })?;
    let cfg: RootConfig = toml::from_str(&body).map_err(|e| ConfigError::ParseToml {
        path: path.to_path_buf(),
        source: e,
    })?;
    let warnings = collect_legacy_alias_warnings(&cfg, &body);
    Ok((cfg, warnings))
}

fn collect_legacy_alias_warnings(_cfg: &RootConfig, body: &str) -> Vec<String> {
    const LEGACY_KEYS: &[&str] = &[
        "persistence_enabled",
        "persistence_path",
        "offline_enabled",
        "offline_ttl",
        "ack_enabled",
        "ack_timeout",
        "ack_retries",
    ];
    LEGACY_KEYS
        .iter()
        .filter(|k| body.contains(&format!("{} =", k)))
        .map(|k| {
            format!(
                "{k} is deprecated; use [persistence]/[persistence.offline]/[ack] section in rex.toml"
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn minimal_toml_parses() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("r.toml");
        std::fs::write(
            &p,
            "[server]\nserver_id=\"x\"\n[[endpoints]]\nprotocol=\"tcp\"\naddress=\"127.0.0.1:9999\"\n",
        )
        .unwrap();
        let (cfg, warnings) = parse_toml(&p).unwrap();
        assert_eq!(cfg.server.server_id, "x");
        assert!(warnings.is_empty());
    }

    #[test]
    fn unknown_key_returns_error() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("r.toml");
        std::fs::write(&p, "[server]\nbogus = 1\n").unwrap();
        let err = parse_toml(&p).unwrap_err();
        assert!(matches!(err, ConfigError::ParseToml { .. }), "got {err:?}");
    }

    #[test]
    fn missing_file_returns_file_not_found() {
        let err = parse_toml(Path::new("/no/such/file.toml")).unwrap_err();
        assert!(
            matches!(err, ConfigError::FileNotFound { .. }),
            "got {err:?}"
        );
    }
}
