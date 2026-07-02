//! Cluster port.
//!
//! Cross-cutting cluster-facing surface: route lookup, node handshake, and
//! message forwarding. `ServerClusterManager` implements this trait.
//!
//! One wide port — split into `ClusterRegistry` (handshake) + `Router`
//! (lookup) is the C4 candidate, deferred until a second consumer appears.

use async_trait::async_trait;

use crate::ForwardRequest;

/// Cluster-facing operations. Mixed sync/async — most methods are sync
/// state queries on the local route table; only `forward_message` is async
/// because it crosses the wire to a remote node.
#[allow(dead_code)] // Port added in commit 6; consumed in commit 7+.
#[async_trait]
pub trait ClusterPort: Send + Sync {
    fn register_client(&self, client_id: u128);
    fn unregister_client(&self, client_id: u128);

    /// Consistent-hash lookup: which node owns the given title?
    fn find_node_for_title(&self, title: &str) -> Option<String>;

    fn get_local_node_id(&self) -> Option<String>;
    fn get_nodes(&self) -> Vec<String>;

    /// Forward a message to a peer node. Returns true on accepted-by-channel.
    async fn forward_message(&self, target_node: &str, request: ForwardRequest) -> bool;
}
