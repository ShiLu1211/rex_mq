//! rex-config: TOML + env + CLI loader for the RexMq broker.
//!
//! Owns the single [`RexConfig`] schema, the four-layer override pipeline,
//! and the strict validator. `rex-server` and `rex-cli` depend on this crate;
//! `rex-config` does not depend on `rex-server`.

pub mod defaults;
pub mod error;
pub mod loader;
pub mod root;
pub mod source;

pub use error::ConfigError;
pub use loader::Loader;
pub use root::RexConfig;
pub use source::cli::CliOverrides;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
