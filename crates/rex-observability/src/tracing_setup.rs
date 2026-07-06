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

/// Install a global tracing subscriber. Idempotent: calling twice in the
/// same process is a no-op (the underlying subscriber detects the duplicate).
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

    result.map_err(|e| TracingError(e.to_string()))
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
