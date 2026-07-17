//! 4-layer loader: defaults → TOML → env → CLI → validate.

use std::path::{Path, PathBuf};

use crate::error::ConfigError;
use crate::root::RexConfig;
use crate::source::cli::CliOverrides;

const DEFAULT_TOML_PATHS: &[&str] = &["./rex.toml", "./config/rex.toml", "/etc/rex/rex.toml"];

#[derive(Debug, Clone, Default)]
pub struct Loader {
    explicit_path: Option<PathBuf>,
    cli_overrides: CliOverrides,
}

impl Loader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_config_path(mut self, p: PathBuf) -> Self {
        self.explicit_path = Some(p);
        self
    }

    pub fn with_config_path_opt(mut self, p: Option<PathBuf>) -> Self {
        self.explicit_path = p;
        self
    }

    pub fn with_cli_overrides(mut self, o: CliOverrides) -> Self {
        self.cli_overrides = o;
        self
    }

    /// Full 4-layer pipeline: defaults → TOML → env → CLI → validate.
    pub fn load(&self) -> Result<RexConfig, ConfigError> {
        // Defaults
        let mut cfg = RexConfig::default();
        let mut warnings: Vec<String> = Vec::new();

        // TOML (optional — None means "no file found, not an error")
        if let Some(path) = self.resolve_path()? {
            let (parsed, parsed_warnings) = crate::source::toml::parse_toml(&path)?;
            cfg = parsed;
            warnings.extend(parsed_warnings);
        }

        // Apply env (best-effort: malformed env-vars warn, never fail)
        let (cfg2, env_warnings) = crate::source::env::apply_env(cfg);
        cfg = cfg2;
        warnings.extend(env_warnings);

        // Apply CLI
        let (cfg3, cli_warnings) = crate::source::cli::apply_cli(cfg, &self.cli_overrides);
        cfg = cfg3;
        warnings.extend(cli_warnings);

        // Validate
        cfg.validate()?;

        for w in &warnings {
            tracing::warn!("{}", w);
        }
        Ok(cfg)
    }

    /// Resolve the config path in this priority:
    ///   (1) `Loader::with_config_path`, if set;
    ///   (2) env `REX_CONFIG`;
    ///   (3) the first existing entry of `DEFAULT_TOML_PATHS`;
    ///   (4) `None` — loader proceeds with defaults + env + CLI only.
    pub fn resolve_path(&self) -> Result<Option<PathBuf>, ConfigError> {
        if let Some(p) = &self.explicit_path {
            if !p.is_file() {
                return Err(ConfigError::FileNotFound { path: p.clone() });
            }
            return Ok(Some(p.clone()));
        }
        if let Ok(p) = std::env::var("REX_CONFIG") {
            let pb = PathBuf::from(p);
            if !pb.is_file() {
                return Err(ConfigError::FileNotFound { path: pb });
            }
            return Ok(Some(pb));
        }
        for candidate in DEFAULT_TOML_PATHS {
            if Path::new(candidate).is_file() {
                return Ok(Some(PathBuf::from(candidate)));
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn explicit_path_wins_over_env_and_search() {
        let tmp = tempdir().unwrap();
        let p = tmp.path().join("explicit.toml");
        std::fs::write(&p, "[server]\nserver_id=\"x\"\n").unwrap();
        let l = Loader::new().with_config_path(p.clone());
        let resolved = l.resolve_path().unwrap().expect("found");
        assert_eq!(resolved, p);
    }

    #[test]
    fn explicit_missing_path_returns_file_not_found() {
        let l = Loader::new().with_config_path(PathBuf::from("/no/such/file.toml"));
        let err = l.resolve_path().unwrap_err();
        assert!(
            matches!(err, ConfigError::FileNotFound { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn no_path_at_all_returns_none() {
        let l = Loader::new();
        let tmp = tempdir().unwrap();
        let original = std::env::current_dir().unwrap();
        std::env::set_current_dir(&tmp).unwrap();
        let result = l.resolve_path().unwrap();
        std::env::set_current_dir(&original).unwrap();
        assert!(result.is_none());
    }
}
