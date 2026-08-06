//! Forwarder port — owns cross-node message delivery.
//!
//! Extracted from `ServerClusterManager` per ADR-0002 (C4 split). Three
//! responsibilities, all async because they touch the cluster wire:
//!
//! - [`Forwarder::forward`] — outbound cluster send; takes the targeted
//!   peer (resolved upstream by the `Router`) and a `ForwardRequest`.
//!   Returns a [`FwdResult`] so callers can branch on actual failure
//!   modes rather than guessing from a bool. The fallback walk over
//!   other known peers lives inside this method.
//! - [`Forwarder::deliver`] — inbound peer-message relay to local
//!   subscribers; returns a [`DeliveryOutcome`] with delivery counts
//!   and the ack-required flag. **ACKs are not broadcast from inside
//!   `deliver`;** the dispatch loop in `ServerClusterManager::handle_messages`
//!   reads the outcome and decides whether to broadcast back. Keeping
//!   the ACK responsibility outside `NetworkForwarder` avoids the need
//!   for a self-reference for the broadcast path.
//! - [`Forwarder::broadcast`] — cluster-internal fan-out (e.g.
//!   `TitleRegister` announcements). Counts successful sends.
//!
//! ## Lifecycle
//!
//! `NetworkForwarder` holds a slot for the cluster's `NodeManager`
//! (`Arc<ArcSwap<Option<Arc<NodeManager>>>>`). The slot is empty at
//! construction and populated when the cluster starts. Calls to
//! `forward` before population return
//! `FwdResult::PeerUnreachable("cluster-not-started")` so callers can
//! detect this state without inspecting internals.
//!
//! ## Module layout
//!
//! - [`mod@network`] — production `Forwarder` impl and the
//!   `try_send` / `try_reconnect_and_send` helpers.
//! - `tests` (cfg(test)) — exhaustive test coverage for the seam,
//!   including direct peer wiring via `recording_listener` and the
//!   unstarted-cluster early-return paths.

use std::sync::Arc;

use arc_swap::ArcSwap;
use async_trait::async_trait;
use rex_cluster::node::NodeManager;
use rex_cluster::route_table::GlobalRouteTable;
use rex_cluster::types::{ClusterMessage, ForwardAckMessage, ForwardMessage};

use crate::ForwardRequest;
use crate::system::client_registry::ClientRegistry;

mod network;

#[cfg(test)]
mod tests;

/// Outcome of an outbound [`Forwarder::forward`] call. Replaces the
/// original `bool` that hid why a send failed (introduced in
/// `handler/title.rs::fallback-loop`).
///
/// The variants expose the failure modes the title handler always
/// wanted but couldn't see. `PeerUnreachable(String)` carries the
/// failing node's name so the fallback loop can skip it without
/// re-asking the route table.
#[derive(Debug, Clone, PartialEq, Eq)]
#[must_use = "FwdResult carries the failure mode; ignoring it loses information"]
pub enum FwdResult {
    /// Message queued on the target node's incoming channel.
    Delivered,
    /// Cluster route table has no node for the targeted title.
    NoPeerForTitle,
    /// The chosen peer is connected but failed to accept the message
    /// (the retry path was exhausted). The inner string is the
    /// failing node's id, so callers can skip it during fallback.
    PeerUnreachable(String),
}

/// Counts and ack state for an inbound [`Forwarder::deliver`] call.
/// Replaces a void return whose outcome could only be read from logs.
///
/// `acked_back` mirrors `ForwardMessage::require_ack` — set `true`
/// when the dispatch loop should broadcast a `ForwardAck`. The actual
/// broadcast lives outside `deliver` (see module docs).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeliveryOutcome {
    /// Number of local subscribers the message was sent to.
    pub delivered_to: usize,
    /// Number of local subscribers where `send_buf` failed.
    pub failed: usize,
    /// Whether the dispatch loop should broadcast a `ForwardAck`
    /// back. Mirrors `ForwardMessage::require_ack`.
    pub acked_back: bool,
}

/// Cross-node message delivery. Split out from `ClusterPort` per
/// ADR-0002 when `cluster/forward_relay.rs` appeared as the second
/// consumer that bypassed the port.
#[async_trait]
pub trait Forwarder: Send + Sync {
    /// Outbound: send `req` to `target_node`. If that peer fails
    /// (transport error), walk all known connected peers — excluding
    /// `target_node` and self — and try each until one accepts.
    async fn forward(&self, target_node: &str, req: &ForwardRequest) -> FwdResult;

    /// Inbound: relay `msg` to local subscribers by title. Counts
    /// deliveries / failures and reports whether the dispatch loop
    /// should broadcast a `ForwardAck`.
    async fn deliver(&self, msg: &ForwardMessage) -> DeliveryOutcome;

    /// Broadcast a `ForwardAck` to all currently connected peers.
    /// Errors are logged; the count of successful sends is the
    /// return value of the symmetric `broadcast` method, but here we
    /// return `()` since the dispatch loop never reads the count.
    async fn announce_ack(&self, ack: &ForwardAckMessage);

    /// Cluster-internal fan-out (e.g. `TitleRegister` announcements).
    /// Returns the count of successful sends.
    async fn broadcast(&self, msg: &ClusterMessage) -> usize;

    /// Whether the cluster has populated the `NodeManager` slot.
    /// Default returns `false` so older / mock implementations stay
    /// trivially constructible.
    fn is_cluster_started(&self) -> bool {
        false
    }
}

/// Production [`Forwarder`] implementation.
///
/// Holds two `ArcSwap` slots — empty at construction, populated by
/// `ServerClusterManager::start` when the cluster comes online:
/// - `node_manager` — the cluster's `NodeManager` (transport + peers).
/// - `route_table` — the cluster's `GlobalRouteTable` (peer lookup).
///
/// Reads use `ArcSwap::load` for an async-lock-free hot path.
pub struct NetworkForwarder {
    node_manager: Arc<ArcSwap<Option<Arc<NodeManager>>>>,
    route_table: Arc<ArcSwap<Option<Arc<GlobalRouteTable>>>>,
    local_node_id: rex_cluster::types::NodeId,
    client_registry: Arc<dyn ClientRegistry>,
}

impl NetworkForwarder {
    /// Construct a new `NetworkForwarder`. Both slots are empty —
    /// populate them via [`NetworkForwarder::set_node_manager`] and
    /// [`NetworkForwarder::set_route_table`] once the cluster has
    /// started. Until then, `forward` returns
    /// `PeerUnreachable("cluster-not-started")`.
    pub fn new(
        node_manager: Arc<ArcSwap<Option<Arc<NodeManager>>>>,
        route_table: Arc<ArcSwap<Option<Arc<GlobalRouteTable>>>>,
        local_node_id: rex_cluster::types::NodeId,
        client_registry: Arc<dyn ClientRegistry>,
    ) -> Arc<Self> {
        Arc::new(Self {
            node_manager,
            route_table,
            local_node_id,
            client_registry,
        })
    }

    /// Populate the cluster's `NodeManager` slot. Called by
    /// `ServerClusterManager::start` when the cluster boots, and
    /// cleared on shutdown.
    pub fn set_node_manager(&self, nm: Option<Arc<NodeManager>>) {
        // `ArcSwap` stores `Arc<T>` internally; wrap the Option so
        // the slot type matches `Arc<Option<Arc<NodeManager>>>`.
        self.node_manager.store(Arc::new(nm));
    }

    /// Populate the cluster's `GlobalRouteTable` slot. Same
    /// lifecycle as `set_node_manager`.
    pub fn set_route_table(&self, rt: Option<Arc<GlobalRouteTable>>) {
        self.route_table.store(Arc::new(rt));
    }

    /// Load the current node manager, if any. The hot path on
    /// `forward` uses this directly.
    fn node_manager(&self) -> Option<Arc<NodeManager>> {
        // `load` returns the inner `Arc<Option<Arc<NodeManager>>>`;
        // `as_ref().clone()` peels one Arc and clones the Option.
        self.node_manager.load().as_ref().clone()
    }

    /// Load the current route table, if any. The hot path on
    /// `forward` uses this directly.
    fn route_table(&self) -> Option<Arc<GlobalRouteTable>> {
        self.route_table.load().as_ref().clone()
    }
}
