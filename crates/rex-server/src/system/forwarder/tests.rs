//! Tests for the [`Forwarder`](super::Forwarder) seam.
//!
//! The forwarder has two surfaces that need distinct test styles:
//!
//! - **Slot population** (`is_cluster_started`, empty-slot early
//!   return on `forward`/`broadcast`) — unit tests with an
//!   unstarted `NetworkForwarder`.
//! - **Wired peer behaviour** (direct send, fallback walk,
//!   `announce_ack` fan-out, `deliver` to local subscribers) — unit
//!   tests that wire a real `NodeManager` + `ClusterTransport`
//!   against a local `TcpListener` (see
//!   [`drain_listener`] / [`recording_listener`] /
//!   [`started_forwarder`]).

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use arc_swap::ArcSwap;
use rex_cluster::types::{ClusterMessage, ForwardAckMessage, ForwardMessage};
use tokio::io::AsyncReadExt;
use tokio::net::TcpListener;
use tokio::sync::mpsc::UnboundedReceiver;
use tokio::time::timeout;

use crate::cluster::forward::{ForwardRequest, ForwardType};
use crate::handler::test_util::dummy_client_with_id;
use crate::system::client_registry::ClientRegistry;
use crate::system::client_registry::ClientRegistryImpl;
use crate::system::forwarder::{DeliveryOutcome, Forwarder, FwdResult, NetworkForwarder};

// ============== Test fixtures ==============

fn empty_forwarder() -> Arc<NetworkForwarder> {
    let nm_slot = Arc::new(ArcSwap::from_pointee(None));
    let rt_slot = Arc::new(ArcSwap::from_pointee(None));
    let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
    NetworkForwarder::new(
        nm_slot,
        rt_slot,
        rex_cluster::types::NodeId::new("local-node"),
        registry,
    )
}

fn sample_forward_request(title: &str) -> ForwardRequest {
    ForwardRequest {
        source_client_id: 0xAAu128,
        target_client_id: 0,
        title: title.to_string(),
        payload: vec![1, 2, 3, 4],
        msg_type: ForwardType::Unicast,
    }
}

// ============== Unstarted-cluster tests ==============

#[tokio::test]
async fn forward_with_unstarted_cluster_returns_cluster_not_started() {
    let forwarder = empty_forwarder();
    let req = sample_forward_request("any_title");
    match forwarder.forward("peer-1", &req).await {
        FwdResult::PeerUnreachable(reason) => {
            assert_eq!(reason, "cluster-not-started");
        }
        other => panic!(
            "expected PeerUnreachable(cluster-not-started), got {:?}",
            other
        ),
    }
}

#[tokio::test]
async fn forward_to_unknown_target_when_unstarted_returns_cluster_not_started() {
    let forwarder = empty_forwarder();
    let req = sample_forward_request("any_title");
    // Empty slot dominates the early-return; even an unknown
    // target returns cluster-not-started.
    let result = forwarder.forward("unknown-target", &req).await;
    assert!(matches!(result, FwdResult::PeerUnreachable(_)));
}

#[tokio::test]
async fn broadcast_with_unstarted_cluster_returns_zero() {
    let forwarder = empty_forwarder();
    let count = forwarder
        .broadcast(&ClusterMessage::Ping(rex_cluster::types::PingMessage {
            node_id: "local-node".into(),
            timestamp: 0,
        }))
        .await;
    assert_eq!(count, 0);
}

// ============== Ack broadcast test ==============

#[tokio::test]
async fn announce_ack_with_transport_walks_connected_nodes() -> anyhow::Result<()> {
    let (peer_a_addr, _listener_a, mut received_a) = recording_listener().await?;
    let (peer_b_addr, _listener_b, mut received_b) = recording_listener().await?;
    let forwarder = started_forwarder(
        "local-node",
        &[("node-a", peer_a_addr, true), ("node-b", peer_b_addr, true)],
    )
    .await;
    let ack = ForwardAckMessage {
        forward_id: 1,
        from_node_id: "local-node".into(),
        original_source: 0xAAu128,
        success: true,
        error: None,
    };

    forwarder.announce_ack(&ack).await;

    // Decode the framed payload (4-byte big-endian length header is
    // already stripped by `recording_listener`) and confirm both
    // connected peers received the same `ForwardAck` we broadcast.
    // The first frame after `transport.connect` is the
    // `ForwardAck` itself — there is no application-level handshake
    // in the current transport — but decoding instead of counting
    // bytes guards against a future handshake frame sneaking in.
    let payload_a = timeout(Duration::from_secs(1), received_a.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for node-a ack"))?
        .ok_or_else(|| anyhow::anyhow!("node-a listener closed before receiving ack"))?;
    let payload_b = timeout(Duration::from_secs(1), received_b.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for node-b ack"))?
        .ok_or_else(|| anyhow::anyhow!("node-b listener closed before receiving ack"))?;
    let msg_a: ClusterMessage = bincode::deserialize(&payload_a)?;
    let msg_b: ClusterMessage = bincode::deserialize(&payload_b)?;
    match msg_a {
        ClusterMessage::ForwardAck(got) => {
            assert_eq!(got.forward_id, ack.forward_id, "node-a ack.forward_id");
            assert_eq!(
                got.from_node_id, ack.from_node_id,
                "node-a ack.from_node_id"
            );
            assert_eq!(
                got.original_source, ack.original_source,
                "node-a ack.original_source"
            );
            assert_eq!(got.success, ack.success, "node-a ack.success");
            assert_eq!(got.error, ack.error, "node-a ack.error");
        }
        other => panic!("node-a expected ClusterMessage::ForwardAck, got {other:?}"),
    }
    match msg_b {
        ClusterMessage::ForwardAck(got) => {
            assert_eq!(got.forward_id, ack.forward_id, "node-b ack.forward_id");
            assert_eq!(
                got.from_node_id, ack.from_node_id,
                "node-b ack.from_node_id"
            );
            assert_eq!(
                got.original_source, ack.original_source,
                "node-b ack.original_source"
            );
            assert_eq!(got.success, ack.success, "node-b ack.success");
            assert_eq!(got.error, ack.error, "node-b ack.error");
        }
        other => panic!("node-b expected ClusterMessage::ForwardAck, got {other:?}"),
    }
    Ok(())
}

// ============== Deliver tests (local subscribers) ==============

#[tokio::test]
async fn deliver_with_no_subscribers_returns_zero_counts_and_no_ack() {
    let forwarder = empty_forwarder();
    let rex_data = rex_core::RexData::new(rex_core::RexCommand::Title, "absent", b"hello");
    let fwd = ForwardMessage {
        forward_id: 1,
        original_source: 0xAAu128,
        target_client_id: 0,
        title: "absent".into(),
        payload: rex_data.pack_ref().to_vec(),
        is_group: false,
        is_broadcast: false,
        require_ack: false,
    };
    let outcome = forwarder.deliver(&fwd).await;
    assert_eq!(outcome.delivered_to, 0);
    assert_eq!(outcome.failed, 0);
    assert!(!outcome.acked_back);
}

#[tokio::test]
async fn deliver_with_local_subscriber_succeeds_and_marks_ack_required() {
    let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
    let client = dummy_client_with_id(0x42u128);
    registry.add_client(client.clone());
    registry.register_title(0x42u128, "delivered_chan");

    let nm_slot = Arc::new(ArcSwap::from_pointee(None));
    let rt_slot = Arc::new(ArcSwap::from_pointee(None));
    let forwarder = NetworkForwarder::new(
        nm_slot,
        rt_slot,
        rex_cluster::types::NodeId::new("local-node"),
        registry,
    );

    let rex_data = rex_core::RexData::new(rex_core::RexCommand::Title, "delivered_chan", b"hello");
    let fwd = ForwardMessage {
        forward_id: 7,
        original_source: 0xAAu128,
        target_client_id: 0,
        title: "delivered_chan".into(),
        payload: rex_data.pack_ref().to_vec(),
        is_group: false,
        is_broadcast: false,
        require_ack: true,
    };

    let outcome = forwarder.deliver(&fwd).await;
    assert_eq!(outcome.delivered_to, 1);
    assert_eq!(outcome.failed, 0);
    assert!(outcome.acked_back, "acked_back should mirror require_ack");
}

#[tokio::test]
async fn deliver_broadcast_delivers_to_all_subscribers() {
    let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
    for id in [0x10u128, 0x20u128, 0x30u128] {
        let c = dummy_client_with_id(id);
        registry.add_client(c.clone());
        registry.register_title(id, "fanout_chan");
    }

    let nm_slot = Arc::new(ArcSwap::from_pointee(None));
    let rt_slot = Arc::new(ArcSwap::from_pointee(None));
    let forwarder = NetworkForwarder::new(
        nm_slot,
        rt_slot,
        rex_cluster::types::NodeId::new("local-node"),
        registry,
    );

    let rex_data = rex_core::RexData::new(rex_core::RexCommand::Title, "fanout_chan", b"hi-all");
    let fwd = ForwardMessage {
        forward_id: 11,
        original_source: 0xAAu128,
        target_client_id: 0,
        title: "fanout_chan".into(),
        payload: rex_data.pack_ref().to_vec(),
        is_group: false,
        is_broadcast: true,
        require_ack: false,
    };

    let outcome = forwarder.deliver(&fwd).await;
    assert_eq!(outcome.delivered_to, 3);
    assert_eq!(outcome.failed, 0);
    assert!(!outcome.acked_back);
}

#[tokio::test]
async fn deliver_with_unknown_client_does_not_panic() {
    let forwarder = empty_forwarder();
    let rex_data = rex_core::RexData::new(rex_core::RexCommand::Title, "absent", b"hello");
    let fwd = ForwardMessage {
        forward_id: 12,
        original_source: 0xAAu128,
        target_client_id: 0xDEADu128,
        title: "absent".into(),
        payload: rex_data.pack_ref().to_vec(),
        is_group: false,
        is_broadcast: false,
        require_ack: false,
    };
    let outcome = forwarder.deliver(&fwd).await;
    assert_eq!(outcome.delivered_to, 0);
}

// ============== Outcome-type sanity tests ==============

#[test]
fn fwd_result_variants_distinct() {
    // Each variant must be a distinct failure mode so handlers
    // can branch on intent.
    let variants = [
        FwdResult::Delivered,
        FwdResult::NoPeerForTitle,
        FwdResult::PeerUnreachable("a".into()),
    ];
    for (i, a) in variants.iter().enumerate() {
        for (j, b) in variants.iter().enumerate() {
            if i != j {
                assert_ne!(a, b);
            }
        }
    }
}

#[test]
fn delivery_outcome_default_is_zero() {
    let o = DeliveryOutcome::default();
    assert_eq!(o.delivered_to, 0);
    assert_eq!(o.failed, 0);
    assert!(!o.acked_back);
}

// ============== Helpers for started-cluster tests ==============
//
// The `Forwarder::forward` contract requires a populated cluster
// (`node_manager` + `route_table`). Spinning up real cluster
// servers would be overkill for unit tests, so we wire a real
// `NodeManager` + `ClusterTransport` against a local TCP listener
// that just drains incoming bytes. The listener accepts the TCP
// connection; `ClusterTransport::connect` inserts the sender into
// its connection map before the wire handshake completes, so
// `is_connected` returns true and `send_to` writes through the
// local channel regardless of what the listener does.

/// Bind a TCP listener on `127.0.0.1:0` and accept connections in
/// the background, draining incoming bytes until the peer closes.
/// Returns the bound address and a handle for abort on drop.
async fn drain_listener() -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind drain listener");
    let addr = listener.local_addr().expect("listener local_addr");
    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                loop {
                    match stream.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(_) => continue,
                    }
                }
            });
        }
    });
    (addr, handle)
}

/// Bind a TCP listener that records each framed cluster message
/// (4-byte big-endian length header already stripped) and forwards
/// the raw payload to the caller via an unbounded mpsc channel.
/// Used to verify that fan-out paths reach every connected peer.
async fn recording_listener() -> anyhow::Result<(
    SocketAddr,
    tokio::task::JoinHandle<()>,
    UnboundedReceiver<Vec<u8>>,
)> {
    let listener = TcpListener::bind("127.0.0.1:0").await?;
    let addr = listener.local_addr()?;
    let (message_tx, message_rx) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        loop {
            let (mut stream, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let message_tx = message_tx.clone();
            tokio::spawn(async move {
                loop {
                    let mut length = [0u8; 4];
                    if stream.read_exact(&mut length).await.is_err() {
                        break;
                    }
                    let mut payload = vec![0u8; u32::from_be_bytes(length) as usize];
                    if stream.read_exact(&mut payload).await.is_err() {
                        break;
                    }
                    let _ = message_tx.send(payload);
                }
            });
        }
    });
    Ok((addr, handle, message_rx))
}

/// Build a started `NetworkForwarder` with a real
/// `NodeManager` + `ClusterTransport` + `GlobalRouteTable`. The
/// caller supplies a list of `(peer_id, peer_addr, should_connect)`
/// tuples. Each peer is registered in the route table; only the
/// peers with `should_connect = true` are wired into the transport
/// connection map. Returns the populated forwarder.
async fn started_forwarder(
    local_id: &str,
    peers: &[(&str, SocketAddr, bool)],
) -> Arc<NetworkForwarder> {
    let nm_slot = Arc::new(ArcSwap::from_pointee(None));
    let rt_slot = Arc::new(ArcSwap::from_pointee(None));
    let registry: Arc<dyn ClientRegistry> = ClientRegistryImpl::new();
    let forwarder = NetworkForwarder::new(
        nm_slot.clone(),
        rt_slot.clone(),
        rex_cluster::types::NodeId::new(local_id),
        registry,
    );

    // NodeManager requires a ClusterMessage channel for inbound
    // forward; the receiver is dropped immediately so any incoming
    // bytes are silently discarded (matches the unit-test pattern
    // used elsewhere in this module).
    let (message_tx, _message_rx) =
        tokio::sync::mpsc::unbounded_channel::<rex_cluster::types::ClusterMessage>();
    let config = rex_cluster::types::ClusterConfig::new(
        rex_cluster::types::NodeId::new(local_id),
        "127.0.0.1:0".parse().expect("parse local listen addr"),
    );
    let nm = Arc::new(rex_cluster::node::NodeManager::new(config, message_tx));
    let transport = nm.get_transport();

    let route_table = rex_cluster::route_table::GlobalRouteTable::with_local_node(
        rex_cluster::types::NodeId::new(local_id),
    );
    for (id, addr, should_connect) in peers {
        route_table.add_node((*id).to_string(), addr.to_string());
        if *should_connect {
            transport
                .connect(rex_cluster::types::NodeId::new(*id), *addr)
                .await
                .expect("transport.connect");
        }
    }

    forwarder.set_node_manager(Some(nm));
    forwarder.set_route_table(Some(Arc::new(route_table)));
    forwarder
}

// ============== forward() direct-send + fallback tests ==============

/// Direct send to a known peer whose address is in the route
/// table. The transport accepts the message, so `forward` returns
/// `Delivered` without walking the fallback list. Guards against
/// regressing the fallback loop (e.g. by triggering it on every
/// successful send).
///
/// Two peers are wired so the fallback walk has at least one
/// candidate. The test asserts not only that the targeted peer
/// (node-b) receives a frame, but also that the other connected
/// peer (node-c) receives nothing — proving the fallback walk was
/// not entered at all. A single-peer wiring would pass even if the
/// fallback walk regressed, because the walk would have no
/// candidate to attempt.
#[tokio::test]
async fn forward_to_known_target_accepted_does_not_fallback() -> anyhow::Result<()> {
    let (peer_b_addr, _listener_b, mut received_b) = recording_listener().await?;
    let (peer_c_addr, _listener_c, mut received_c) = recording_listener().await?;
    let fwd = started_forwarder(
        "local-node",
        &[("node-b", peer_b_addr, true), ("node-c", peer_c_addr, true)],
    )
    .await;
    let req = sample_forward_request("news");

    let r = fwd.forward("node-b", &req).await;
    assert!(matches!(r, FwdResult::Delivered), "got {r:?}");

    // Targeted peer must have received exactly one framed payload.
    let payload_b = timeout(Duration::from_secs(1), received_b.recv())
        .await
        .map_err(|_| anyhow::anyhow!("timed out waiting for node-b frame"))?
        .ok_or_else(|| anyhow::anyhow!("node-b listener closed before receiving frame"))?;
    let msg_b: ClusterMessage = bincode::deserialize(&payload_b)?;
    match msg_b {
        ClusterMessage::Forward(_) => {}
        other => panic!("node-b expected ClusterMessage::Forward, got {other:?}"),
    }

    // The fallback walk must not have attempted node-c. Use a
    // short timeout — the absence of a frame is the assertion.
    let frame_c = timeout(Duration::from_millis(100), received_c.recv()).await;
    assert!(
        frame_c.is_err(),
        "node-c received an unexpected frame (fallback walk entered): {frame_c:?}"
    );
    Ok(())
}

/// When the targeted peer is registered in the route table but
/// has no live transport connection, the direct path fails and
/// `forward` must walk the other connected peers. Today `title.rs`
/// hand-rolls this loop; this test guards `Forwarder::forward`
/// from regressing without it.
#[tokio::test]
async fn forward_target_refused_falls_back_to_other_peer() {
    // node-b is registered in the route table but NEVER connected
    // (so `is_connected("node-b")` is false and the direct send
    // fails). node-c is connected and accepts the message; the
    // fallback walk should land on it.
    let (peer_c_addr, _listener_c) = drain_listener().await;
    let unreachable: SocketAddr = "127.0.0.1:1".parse().expect("parse unreachable");
    let fwd = started_forwarder(
        "local-node",
        &[
            ("node-b", unreachable, false),
            ("node-c", peer_c_addr, true),
        ],
    )
    .await;
    let req = sample_forward_request("news");

    let r = fwd.forward("node-b", &req).await;
    assert!(matches!(r, FwdResult::Delivered), "got {r:?}");
}

/// When the targeted peer is registered in the route table but
/// the route table's address string is unparseable as a SocketAddr,
/// `forward` must log and walk the fallback list rather than panic
/// or return a misleading `NoPeerForTitle`. This is the regression
/// case for any future route-table hand-off that drops the
/// `parse::<SocketAddr>` guard.
#[tokio::test]
async fn forward_with_unparseable_route_addr_falls_back_to_known_peers() {
    // node-f is connected via the started_forwarder helper, then we
    // manually inject node-b with a garbage addr into the route
    // table. We bypass `started_forwarder` because it requires
    // SocketAddr in its peer tuple.
    let (peer_f_addr, _listener_f) = drain_listener().await;
    let fwd = started_forwarder("local-node", &[("node-f", peer_f_addr, true)]).await;
    // Inject the bad addr post-construction.
    let rt_arc = fwd.route_table().expect("route table set");
    rt_arc.add_node("node-b".to_string(), "not-a-valid-addr".to_string());

    let req = sample_forward_request("news");
    let r = fwd.forward("node-b", &req).await;
    assert!(
        matches!(r, FwdResult::Delivered),
        "expected Delivered via fallback walk, got {r:?}"
    );
}

/// When the cluster is started but `connected_nodes()` is empty and
/// the target is not in the route table, `forward` returns
/// `NoPeerForTitle` — no panic, no silent success, no fallback
/// walk to attempt.
#[tokio::test]
async fn forward_with_empty_connected_nodes_returns_no_peer_for_title() {
    let fwd = started_forwarder("local-node", &[]).await;
    let req = sample_forward_request("any_title");
    let r = fwd.forward("unknown-peer", &req).await;
    assert!(
        matches!(r, FwdResult::NoPeerForTitle),
        "expected NoPeerForTitle, got {r:?}"
    );
}
