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
    /// List clients currently connected. Used by `/admin/clients` (Task 7)
    /// and replaced with a real implementation in Task 8. The default
    /// returns an empty vec so existing adapters keep compiling.
    fn list_clients(&self) -> Vec<ClientSummary> {
        Vec::new()
    }
}

/// Plain-data snapshot of a single connected client. Returned by
/// [`RegistrySnapshot::list_clients`]. Bridges the rex-server
/// `ClientRegistry` port (u128 ids) to the observability probes without
/// a rex-server dependency.
#[derive(Debug, Clone)]
pub struct ClientSummary {
    pub id: u128,
    pub transport: String,
    pub titles: Vec<String>,
    pub connected_secs: u64,
}

/// Hook used by `/admin/clients/:id/disconnect` (Task 10). Implementing
/// adapters call the live client's cancel signal. `None` causes the
/// endpoint to return 503.
pub trait ClientCancel: Send + Sync {
    fn cancel(&self, client_id: u64);
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
