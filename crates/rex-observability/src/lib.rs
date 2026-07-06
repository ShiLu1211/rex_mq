//! Observability framework: metrics, tracing, health, admin.

pub mod admin;
pub mod health;
pub mod http;
pub mod metrics;
pub mod probe;
pub mod tracing_setup;

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
