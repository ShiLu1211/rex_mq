//! Error type returned from [`crate::Loader::load`].

use std::path::PathBuf;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("config file not found at explicit path {path:?}")]
    FileNotFound { path: PathBuf },

    #[error("config file {path:?}: {source}")]
    ParseToml {
        path: PathBuf,
        source: toml::de::Error,
    },

    #[error("unknown config key: {section}.{key}")]
    UnknownKey { section: String, key: String },

    #[error("invalid value at {section}.{key}: {value:?} ({reason})")]
    InvalidValue {
        section: String,
        key: String,
        value: String,
        reason: String,
    },

    #[error("missing required field: {section}.{key}")]
    MissingRequired { section: String, key: String },

    #[error("semantic: {0}")]
    Semantic(String),

    #[error("io error reading {path:?}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io {
            path: PathBuf::from("<unknown>"),
            source: e,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_semantic() {
        let e = ConfigError::Semantic("cluster requires cluster_addr".into());
        assert_eq!(e.to_string(), "semantic: cluster requires cluster_addr");
    }

    #[test]
    fn display_unknown_key() {
        let e = ConfigError::UnknownKey {
            section: "server".into(),
            key: "bogus".into(),
        };
        assert_eq!(e.to_string(), "unknown config key: server.bogus");
    }

    #[test]
    fn from_io_error() {
        let ioe = std::io::Error::new(std::io::ErrorKind::NotFound, "no such file");
        let e: ConfigError = ioe.into();
        assert!(matches!(e, ConfigError::Io { .. }));
    }
}
