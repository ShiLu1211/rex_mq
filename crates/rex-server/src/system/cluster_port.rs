//! Cluster port.
//!
//! Cluster-facing membership surface: route lookup, node handshake, and
//! client registration. Wire-level message delivery (forwarding and
//! broadcasting `ClusterMessage`s) lives on the `Forwarder` port per
//! ADR-0003. `ServerClusterManager` implements this trait.
//!
//! One wide port — split into `ClusterRegistry` (handshake) + `Router`
//! (lookup) is the C4 candidate, deferred until a second consumer appears.

use async_trait::async_trait;

/// Cluster-facing membership operations. All methods are sync state
/// queries on the local route table — wire I/O lives on `Forwarder`.
#[async_trait]
pub trait ClusterPort: Send + Sync {
    fn register_client(&self, client_id: u128);
    fn unregister_client(&self, client_id: u128);

    /// Consistent-hash lookup: which node owns the given title?
    fn find_node_for_title(&self, title: &str) -> Option<String>;

    fn get_local_node_id(&self) -> Option<String>;
    fn get_nodes(&self) -> Vec<String>;
}
