//! HealthProbe port + aggregator.

use std::sync::Arc;

pub trait HealthProbe: Send + Sync {
    fn name(&self) -> &'static str;
    fn check(&self) -> ProbeResult;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeResult {
    Healthy,
    Degraded { reason: String },
    Unhealthy { reason: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AggregateStatus {
    Healthy,
    Degraded,
    Unhealthy,
}

#[derive(Debug, Clone)]
pub struct AggregatedHealth {
    pub status: AggregateStatus,
    pub results: Vec<(&'static str, ProbeResult)>,
}

#[derive(Default)]
pub struct HealthRegistry {
    probes: Vec<Arc<dyn HealthProbe>>,
}

impl HealthRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(&mut self, probe: Arc<dyn HealthProbe>) {
        self.probes.push(probe);
    }

    pub fn check_all(&self) -> AggregatedHealth {
        let mut status = AggregateStatus::Healthy;
        let mut results = Vec::with_capacity(self.probes.len());
        for probe in &self.probes {
            // catch_unwind prevents a misbehaving probe from breaking /readyz.
            // AssertUnwindSafe is required because the trait object only
            // guarantees Send + Sync, not UnwindSafe.
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| probe.check()))
                .unwrap_or_else(|_| ProbeResult::Unhealthy {
                    reason: "probe_panicked".into(),
                });
            match (&result, &status) {
                (ProbeResult::Unhealthy { .. }, _) => status = AggregateStatus::Unhealthy,
                (ProbeResult::Degraded { .. }, AggregateStatus::Healthy) => {
                    status = AggregateStatus::Degraded;
                }
                _ => {}
            }
            results.push((probe.name(), result));
        }
        AggregatedHealth { status, results }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct HealthyProbe;
    impl HealthProbe for HealthyProbe {
        fn name(&self) -> &'static str {
            "ok"
        }
        fn check(&self) -> ProbeResult {
            ProbeResult::Healthy
        }
    }

    struct DegradedProbe;
    impl HealthProbe for DegradedProbe {
        fn name(&self) -> &'static str {
            "warn"
        }
        fn check(&self) -> ProbeResult {
            ProbeResult::Degraded {
                reason: "slow".into(),
            }
        }
    }

    struct UnhealthyProbe;
    impl HealthProbe for UnhealthyProbe {
        fn name(&self) -> &'static str {
            "down"
        }
        fn check(&self) -> ProbeResult {
            ProbeResult::Unhealthy {
                reason: "dead".into(),
            }
        }
    }

    #[test]
    fn aggregates_all_healthy() {
        let mut r = HealthRegistry::new();
        r.register(Arc::new(HealthyProbe));
        r.register(Arc::new(HealthyProbe));
        let agg = r.check_all();
        assert_eq!(agg.status, AggregateStatus::Healthy);
        assert_eq!(agg.results.len(), 2);
    }

    #[test]
    fn degraded_does_not_demote_to_unhealthy() {
        let mut r = HealthRegistry::new();
        r.register(Arc::new(HealthyProbe));
        r.register(Arc::new(DegradedProbe));
        let agg = r.check_all();
        assert_eq!(agg.status, AggregateStatus::Degraded);
    }

    #[test]
    fn unhealthy_wins_over_degraded() {
        let mut r = HealthRegistry::new();
        r.register(Arc::new(DegradedProbe));
        r.register(Arc::new(UnhealthyProbe));
        let agg = r.check_all();
        assert_eq!(agg.status, AggregateStatus::Unhealthy);
    }
}
