use super::traits::RegistrySnapshot;
use crate::health::{HealthProbe, ProbeResult};

pub struct RegistryHealthProbe {
    inner: std::sync::Arc<dyn RegistrySnapshot>,
}

impl RegistryHealthProbe {
    pub fn new(inner: std::sync::Arc<dyn RegistrySnapshot>) -> Self {
        Self { inner }
    }
}

impl HealthProbe for RegistryHealthProbe {
    fn name(&self) -> &'static str {
        "registry"
    }
    fn check(&self) -> ProbeResult {
        let max = self.inner.max_clients();
        let count = self.inner.client_count();
        if max > 0 && count >= max {
            return ProbeResult::Unhealthy {
                reason: format!("client_count {} >= max {}", count, max),
            };
        }
        ProbeResult::Healthy
    }
}
