//! Observability framework: metrics, tracing, health, admin.

pub mod admin;
pub mod config;
pub mod health;
pub mod http;
pub mod metrics;
pub mod probe;
pub mod tracing_setup;

pub use config::ObservabilityConfig;

use std::sync::Arc;

use crate::health::HealthRegistry;
use crate::http::{ServerHandle, serve};

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// Handle to a started observability stack. Drop the value (or call
/// `shutdown`) to stop the HTTP server.
pub struct ObservabilityHandle {
    pub http: ServerHandle,
    pub health: Arc<HealthRegistry>,
}

impl ObservabilityHandle {
    /// Start the observability stack:
    /// 1. Install the global tracing subscriber.
    /// 2. Use the caller-supplied `HealthRegistry` so probes registered
    ///    after `start` and the `/readyz` endpoint see the same state.
    /// 3. Build the admin `AdminState` with the supplied registry /
    ///    cancel adapters (or `None` for tests).
    /// 4. Spawn the admin HTTP server on `cfg.admin_addr`.
    ///
    /// Returns `anyhow::Result` so callers can use `?` inside
    /// `async fn` returns of `anyhow::Result`. The brief originally
    /// specified `Box<dyn Error>` but that doesn't satisfy the
    /// `Send + Sync + 'static` bounds `anyhow` requires for `?`.
    pub fn start(
        cfg: &ObservabilityConfig,
        health: Arc<HealthRegistry>,
        registry: Option<Arc<dyn probe::traits::RegistrySnapshot>>,
        client_cancel: Option<Arc<dyn probe::traits::ClientCancel>>,
    ) -> anyhow::Result<Self> {
        tracing_setup::init_tracing(cfg.tracing_format).map_err(|e| anyhow::anyhow!("{}", e))?;
        let admin_state = admin::AdminState {
            health: health.clone(),
            admin: admin::AdminConfig {
                token: cfg.admin_token.clone(),
            },
            registry,
            client_cancel,
        };
        let router = admin::build_router_with_state(admin_state);
        let http = futures_executor::block_on(serve(router, cfg.admin_addr))?;
        Ok(Self { http, health })
    }

    /// Trigger graceful shutdown of the admin HTTP server. Blocks until
    /// the server task completes its cleanup.
    pub fn shutdown(self) {
        self.http.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_non_empty() {
        assert!(!version().is_empty());
    }
}
