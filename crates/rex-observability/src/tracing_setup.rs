//! tracing-subscriber initialization for rex-server.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TracingFormat {
    Json,
    Pretty,
}

impl fmt::Display for TracingFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TracingFormat::Json => write!(f, "json"),
            TracingFormat::Pretty => write!(f, "pretty"),
        }
    }
}

#[derive(Debug)]
pub struct TracingError(pub String);

impl std::fmt::Display for TracingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "tracing init failed: {}", self.0)
    }
}

impl std::error::Error for TracingError {}

/// Install a global tracing subscriber. Idempotent: if a global
/// subscriber has already been set (e.g. by an earlier test call or
/// a CLI bootstrap), this returns `Ok(())` instead of erroring.
/// Tests rely on this — the `rex-test` factory calls
/// `tracing_subscriber::fmt::try_init` and then `open_server` runs
/// through here a second time.
pub fn init_tracing(format: TracingFormat) -> Result<(), TracingError> {
    let env_filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    let result = match format {
        TracingFormat::Json => tracing_subscriber::fmt()
            .json()
            .with_env_filter(env_filter)
            .try_init(),
        TracingFormat::Pretty => tracing_subscriber::fmt()
            .pretty()
            .with_env_filter(env_filter)
            .try_init(),
    };

    // `try_init` returns Err only when a global subscriber has
    // already been installed. Treat that as a no-op so calling
    // `open_server` twice (or after the test factory's bootstrap)
    // doesn't fail. Real errors other than the duplicate-set case
    // don't exist for `try_init` — only the global-set error is
    // returned.
    match result {
        Ok(()) => Ok(()),
        Err(_) => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_format() {
        assert_eq!(TracingFormat::Json.to_string(), "json");
        assert_eq!(TracingFormat::Pretty.to_string(), "pretty");
    }

    // We can't actually test init_tracing success in a unit test because the
    // global subscriber can only be installed once per process. The
    // double-init path is exercised in integration tests instead.
}
