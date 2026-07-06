//! Local mirrors of the port traits rex-observability probes consult.
//!
//! rex-observability must not depend on rex-server, so the probes see these
//! narrow surfaces. In rex-server, write thin adapter impls (one per port)
//! that delegate to the real Services fields.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub trait RegistrySnapshot: Send + Sync {
    fn client_count(&self) -> usize;
    fn title_count(&self) -> usize;
    /// Optional: a max-clients threshold; 0 = no limit.
    fn max_clients(&self) -> usize {
        0
    }
}

pub trait ClusterSnapshot: Send + Sync {
    fn peer_count(&self) -> usize;
    fn local_node_present(&self) -> bool;
}

pub trait PersistenceSnapshot: Send + Sync {
    /// Returns the last error the persistence layer observed, if any.
    fn last_error(&self) -> Option<String>;
}

pub trait ForwarderSnapshot: Send + Sync {
    /// True iff the NodeManager slot is populated (cluster started).
    fn node_manager_ready(&self) -> bool;
}

// Shared atomic-flag helper used by PersistenceHealthProbe.
#[derive(Default)]
pub struct AtomicHealth {
    healthy: AtomicBool,
}

impl AtomicHealth {
    pub fn mark_healthy(&self) {
        self.healthy.store(true, Ordering::Release);
    }
    pub fn mark_unhealthy(&self) {
        self.healthy.store(false, Ordering::Release);
    }
    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Acquire)
    }
}

// Re-export Arc convenience.
pub type Shared<T> = Arc<T>;
