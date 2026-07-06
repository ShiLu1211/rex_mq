use super::traits::ForwarderSnapshot;
use crate::health::{HealthProbe, ProbeResult};

pub struct ForwarderHealthProbe {
    inner: std::sync::Arc<dyn ForwarderSnapshot>,
}

impl ForwarderHealthProbe {
    pub fn new(inner: std::sync::Arc<dyn ForwarderSnapshot>) -> Self {
        Self { inner }
    }
}

impl HealthProbe for ForwarderHealthProbe {
    fn name(&self) -> &'static str {
        "forwarder"
    }
    fn check(&self) -> ProbeResult {
        if self.inner.node_manager_ready() {
            ProbeResult::Healthy
        } else {
            ProbeResult::Degraded {
                reason: "cluster not started".into(),
            }
        }
    }
}
