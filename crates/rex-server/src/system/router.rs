//! Router port — answers "where does this title go?"
//!
//! Before C4, `handler/title.rs` had 55 lines of local-first / remote-fallback
//! / try-all-nodes logic. The answer to "where does a title route to?" was
//! reconstructed by reading five levels of delegation. The Router sucks that
//! logic behind one seam.
//!
//! `ClusterRouter` is the production implementation — it checks the local
//! registry first (`ClientRegistry::find_one_by_title`), then falls back to
//! the cluster route table (`ClusterPort::find_node_for_title`). `forward`
//! delegates to `ClusterPort::forward_message`.
//!
//! Two adapters justify the seam:
//! - `ClusterRouter` (prod, DashMap + hash-ring)
//! - A test double that returns canned `RoutePlan` values

use std::sync::Arc;

use async_trait::async_trait;
use rex_core::RexClientInner;

use crate::ForwardRequest;
use crate::system::client_registry::ClientRegistry;
use crate::system::cluster_port::ClusterPort;

/// The three possible destinations for a title.
pub enum RoutePlan {
    /// Deliver locally to this client.
    Local(Arc<RexClientInner>),
    /// Forward to the named peer node.
    Remote(String),
    /// No target known — neither locally nor remotely.
    None,
}

/// Answers routing questions. `forward` is async because it crosses the
/// wire to a peer node; `route` and the accessors are sync.
#[async_trait]
pub trait Router: Send + Sync {
    /// Resolve a title to a delivery plan. `exclude` is the sender's client
    /// id (so we don't route back to ourselves).
    fn route(&self, title: &str, exclude: Option<u128>) -> RoutePlan;

    /// Forward a message to a peer node. Returns true on accepted-by-channel.
    async fn forward(&self, target_node: &str, request: ForwardRequest) -> bool;

    /// The local node's id, if the cluster is active.
    fn local_node_id(&self) -> Option<String>;

    /// All known peer nodes (including the local node). Useful for
    /// broadcast-fallback when the primary route fails.
    fn known_nodes(&self) -> Vec<String>;
}

/// Production implementation: local registry + cluster route table.
pub struct ClusterRouter {
    registry: Arc<dyn ClientRegistry>,
    cluster: Arc<dyn ClusterPort>,
}

impl ClusterRouter {
    pub fn new(registry: Arc<dyn ClientRegistry>, cluster: Arc<dyn ClusterPort>) -> Arc<Self> {
        Arc::new(Self { registry, cluster })
    }
}

#[async_trait]
impl Router for ClusterRouter {
    fn route(&self, title: &str, exclude: Option<u128>) -> RoutePlan {
        // Local subscriber has priority.
        if let Some(client) = self.registry.find_one_by_title(title, exclude) {
            return RoutePlan::Local(client);
        }
        // Try the cluster route table.
        if let Some(node) = self.cluster.find_node_for_title(title) {
            return RoutePlan::Remote(node);
        }
        RoutePlan::None
    }

    async fn forward(&self, target_node: &str, request: ForwardRequest) -> bool {
        self.cluster.forward_message(target_node, request).await
    }

    fn local_node_id(&self) -> Option<String> {
        self.cluster.get_local_node_id()
    }

    fn known_nodes(&self) -> Vec<String> {
        self.cluster.get_nodes()
    }
}

// ---- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ForwardType;
    use crate::handler::test_util::{TestClusterPort, TestRegistry, dummy_client_with_id};

    #[test]
    fn route_returns_local_when_subscriber_present() {
        let registry = TestRegistry::new();
        let cluster: Arc<dyn ClusterPort> = Arc::new(TestClusterPort::new());
        let router = ClusterRouter::new(registry.to_arc(), cluster);

        let target = dummy_client_with_id(0x42u128);
        registry.add_client(target.clone());
        registry.register_title(0x42u128, "local_chan");

        let plan = router.route("local_chan", None);
        match plan {
            RoutePlan::Local(client) => assert_eq!(client.id(), 0x42u128),
            _ => panic!("expected Local"),
        }
    }

    #[test]
    fn route_returns_remote_when_cluster_has_node() {
        let registry = TestRegistry::new();
        let mut cluster = TestClusterPort::new();
        cluster.find_node_for_title = Some("peer-1".to_string());
        let cluster: Arc<dyn ClusterPort> = Arc::new(cluster);
        let router = ClusterRouter::new(registry.to_arc(), cluster);

        let plan = router.route("remote_chan", None);
        match plan {
            RoutePlan::Remote(node) => assert_eq!(node, "peer-1"),
            _ => panic!("expected Remote"),
        }
    }

    #[test]
    fn route_prefers_local_over_remote() {
        let registry = TestRegistry::new();
        let target = dummy_client_with_id(0x99u128);
        registry.add_client(target.clone());
        registry.register_title(0x99u128, "both_chan");

        let mut cluster = TestClusterPort::new();
        cluster.find_node_for_title = Some("peer-1".to_string());
        let cluster: Arc<dyn ClusterPort> = Arc::new(cluster);
        let router = ClusterRouter::new(registry.to_arc(), cluster);

        // Local subscriber should shadow the cluster entry.
        let plan = router.route("both_chan", None);
        match plan {
            RoutePlan::Local(client) => assert_eq!(client.id(), 0x99u128),
            _ => panic!("expected Local, got remote or none"),
        }
    }

    #[test]
    fn route_returns_none_when_no_match() {
        let registry = TestRegistry::new();
        let cluster: Arc<dyn ClusterPort> = Arc::new(TestClusterPort::new());
        let router = ClusterRouter::new(registry.to_arc(), cluster);

        let plan = router.route("nobody_here", None);
        assert!(matches!(plan, RoutePlan::None));
    }

    #[test]
    fn route_excludes_sender() {
        let registry = TestRegistry::new();
        let sender = dummy_client_with_id(0x3u128);
        let target = dummy_client_with_id(0x4u128);
        registry.add_client(sender.clone());
        registry.add_client(target.clone());
        registry.register_title(0x3u128, "echo_chan");
        registry.register_title(0x4u128, "echo_chan");

        let cluster: Arc<dyn ClusterPort> = Arc::new(TestClusterPort::new());
        let router = ClusterRouter::new(registry.to_arc(), cluster);

        // Excluding 3 means only 4 should be returned.
        let plan = router.route("echo_chan", Some(0x3u128));
        match plan {
            RoutePlan::Local(client) => assert_eq!(client.id(), 0x4u128),
            _ => panic!("expected Local(4)"),
        }
    }
}
