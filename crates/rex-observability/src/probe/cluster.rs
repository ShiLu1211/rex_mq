use super::traits::ClusterSnapshot;
use crate::health::{HealthProbe, ProbeResult};

pub struct ClusterHealthProbe {
    inner: std::sync::Arc<dyn ClusterSnapshot>,
    /// If true, single-node deployments skip the peer-count check.
    pub single_node_ok: bool,
}

impl ClusterHealthProbe {
    pub fn new(inner: std::sync::Arc<dyn ClusterSnapshot>, single_node_ok: bool) -> Self {
        Self {
            inner,
            single_node_ok,
        }
    }
}

impl HealthProbe for ClusterHealthProbe {
    fn name(&self) -> &'static str {
        "cluster"
    }
    fn check(&self) -> ProbeResult {
        if !self.inner.local_node_present() {
            return ProbeResult::Unhealthy {
                reason: "local node not registered".into(),
            };
        }
        let peers = self.inner.peer_count();
        if peers == 0 && !self.single_node_ok {
            return ProbeResult::Degraded {
                reason: "no cluster peers (single-node?)".into(),
            };
        }
        ProbeResult::Healthy
    }
}
