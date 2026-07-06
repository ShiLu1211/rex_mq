use super::traits::PersistenceSnapshot;
use crate::health::{HealthProbe, ProbeResult};

pub struct PersistenceHealthProbe {
    inner: std::sync::Arc<dyn PersistenceSnapshot>,
}

impl PersistenceHealthProbe {
    pub fn new(inner: std::sync::Arc<dyn PersistenceSnapshot>) -> Self {
        Self { inner }
    }
}

impl HealthProbe for PersistenceHealthProbe {
    fn name(&self) -> &'static str {
        "persistence"
    }
    fn check(&self) -> ProbeResult {
        match self.inner.last_error() {
            Some(err) => ProbeResult::Degraded { reason: err },
            None => ProbeResult::Healthy,
        }
    }
}
