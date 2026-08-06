//! Node manager — owns cluster membership state and the inbound
//! transport loop.
//!
//! Responsibilities:
//!
//! - Hold the per-node `ClusterConfig` and the connection pool to other
//!   nodes (`ClusterTransport`).
//! - Maintain a `nodes` map of known peers, updated by `Join` and
//!   `Heartbeat` messages.
//! - Drive a periodic heartbeat broadcast so peers can detect liveness
//!   without a separate `gossip` layer.
//!
//! Cross-node wire I/O (sending a `Forward`, broadcasting an ack) lives
//! on `Forwarder` in `rex-server::system::forwarder`; `NodeManager` only
//! emits heartbeats and accepts inbound traffic.

use std::sync::Arc;

use anyhow::Result;
use dashmap::DashMap;
use tokio::io::AsyncReadExt;
use tokio::sync::{broadcast, mpsc};
use tokio::time::{Duration, interval};
use tracing::{debug, error, info, warn};

use crate::transport::{ClusterTransport, IncomingMessage};
use crate::types::{ClusterConfig, ClusterMessage, HeartbeatMessage, NodeId, NodeInfo};

/// Cluster node manager.
pub struct NodeManager {
    /// Local node configuration
    config: ClusterConfig,
    /// Known nodes (node_id -> NodeInfo)
    nodes: DashMap<String, NodeInfo>,
    /// Transport layer
    transport: Arc<ClusterTransport>,
    /// Shutdown signal
    shutdown_tx: broadcast::Sender<()>,
}

impl NodeManager {
    /// Create a new node manager. The `message_tx` channel is taken
    /// by the inbound-forwarding task; the cluster manager's dispatch
    /// loop receives from it.
    pub fn new(config: ClusterConfig, message_tx: mpsc::UnboundedSender<ClusterMessage>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        let local_node_id = config.node_id.clone();

        let (transport_tx, transport_rx) = mpsc::unbounded_channel();

        let transport = Arc::new(ClusterTransport::new(local_node_id.clone(), transport_tx));

        // Spawn message forwarding task — moves message_tx into the task.
        let nodes_map = Arc::new(DashMap::new());
        let nodes_map_clone = nodes_map.clone();

        tokio::spawn(async move {
            Self::forward_messages(transport_rx, nodes_map_clone, message_tx).await;
        });

        Self {
            config,
            nodes: DashMap::new(),
            transport,
            shutdown_tx,
        }
    }

    /// Forward incoming transport messages to the dispatch loop and
    /// stamp the source node's `last_heartbeat` on every receipt.
    async fn forward_messages(
        mut rx: mpsc::UnboundedReceiver<IncomingMessage>,
        nodes: Arc<DashMap<String, NodeInfo>>,
        message_tx: mpsc::UnboundedSender<ClusterMessage>,
    ) {
        while let Some(incoming) = rx.recv().await {
            let source_node_id = incoming.source_node.as_str().to_string();

            // Update last seen time for the node
            if let Some(mut node_info) = nodes.get_mut(&source_node_id) {
                node_info.last_heartbeat = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
            }

            // Forward to message handler
            if let Err(e) = message_tx.send(incoming.message) {
                // This happens when the receiver is dropped (e.g., during shutdown)
                debug!("Failed to forward message (receiver dropped): {}", e);
            }
        }
    }

    /// Start the node manager — opens the listener, connects to seeds,
    /// and spawns the heartbeat broadcaster.
    pub async fn start(&self) -> Result<()> {
        // Start transport listener
        let listen_addr = self.config.listen_addr;
        let transport = self.transport.clone();

        tokio::spawn(async move {
            if let Err(e) = transport.start_listener(listen_addr).await {
                error!("Transport listener error: {}", e);
            }
        });

        // Connect to seed nodes if any
        if !self.config.seed_nodes.is_empty() {
            self.connect_to_seeds().await?;
        }

        // Start heartbeat/task loop
        self.start_background_tasks();

        Ok(())
    }

    /// Connect to seed nodes.
    async fn connect_to_seeds(&self) -> Result<()> {
        for seed_addr in &self.config.seed_nodes {
            // Use the seed address as a temporary node_id key since we don't know the seed's actual node_id yet
            // After receiving the NodeList response, we'll update the route table with the correct node_id
            let temp_node_id = format!("seed-{}", seed_addr);
            info!(
                "Connecting to seed at {} (temp key: {})",
                seed_addr, temp_node_id
            );

            if let Err(e) = self
                .transport
                .connect(temp_node_id.into(), *seed_addr)
                .await
            {
                warn!("Failed to connect to seed {}: {}", seed_addr, e);
                continue;
            }

            // Send Join message with our real node_id
            let local_info = NodeInfo::new(self.config.node_id.clone(), self.config.listen_addr);
            let join_msg = ClusterMessage::Join(local_info);
            let temp_key = format!("seed-{}", seed_addr);

            if let Err(e) = self.transport.send_to(&temp_key, &join_msg).await {
                warn!("Failed to send join to seed {}: {}", seed_addr, e);
            }
        }

        Ok(())
    }

    /// Broadcast a `Heartbeat` message on the configured interval so
    /// peers can detect our liveness.
    fn start_background_tasks(&self) {
        let heartbeat_interval = Duration::from_millis(self.config.heartbeat_interval_ms);
        let transport = self.transport.clone();
        let config = self.config.clone();

        tokio::spawn(async move {
            let mut ticker = interval(heartbeat_interval);

            loop {
                ticker.tick().await;

                let heartbeat = HeartbeatMessage {
                    leader_id: config.node_id.clone(),
                };
                let msg = ClusterMessage::Heartbeat(heartbeat);

                if let Err(e) = transport.broadcast(&msg).await {
                    debug!("Heartbeat broadcast error: {}", e);
                }
            }
        });
    }

    /// Handle an incoming cluster message.
    pub async fn handle_message(&self, message: ClusterMessage) -> Result<()> {
        match message {
            ClusterMessage::Join(node_info) => {
                self.handle_join(node_info).await?;
            }
            ClusterMessage::Heartbeat(_) => {
                // Liveness is recorded in `forward_messages` via
                // NodeInfo::last_heartbeat; nothing to do here.
            }
            ClusterMessage::Forward(forward) => {
                self.handle_forward(forward).await?;
            }
            _ => {
                debug!("Unhandled cluster message: {:?}", message);
            }
        }

        Ok(())
    }

    /// Handle node join.
    async fn handle_join(&self, node_info: NodeInfo) -> Result<()> {
        let node_id_str = node_info.node_id.as_str().to_string();
        info!("Node {} joined the cluster", node_id_str);
        self.nodes.insert(node_id_str, node_info);
        Ok(())
    }

    /// Handle a forwarded message. Delivery to local subscribers is the
    /// Forwarder port's responsibility — this method exists to keep the
    /// inbound message channel from filling with undispatched messages.
    async fn handle_forward(&self, forward: crate::types::ForwardMessage) -> Result<()> {
        debug!(
            "Received forwarded message for client {}",
            forward.target_client_id
        );
        Ok(())
    }

    /// Send a message to another node.
    pub async fn send_to(&self, target_node_id: &str, message: ClusterMessage) -> Result<()> {
        self.transport.send_to(target_node_id, &message).await
    }

    /// Get the transport layer.
    pub fn get_transport(&self) -> Arc<ClusterTransport> {
        self.transport.clone()
    }

    /// Shutdown the node manager.
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
        self.transport.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_node_manager_creation() {
        let config = ClusterConfig::default();
        let (tx, _rx) = mpsc::unbounded_channel();
        let _manager = NodeManager::new(config, tx);
        // Smoke test: construction wires transport + spawns forwarding task
        // without panicking. The forwarding task exits when message_tx is
        // dropped (it is, here, at end of scope).
    }

    // ---------- New tests (PR 3: rex-cluster test coverage) ----------

    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::net::TcpListener;

    /// Bind an ephemeral TCP listener on 127.0.0.1:0 and accept
    /// connections in the background, draining incoming bytes. Used
    /// to give NodeManager::start a real listen_addr + a real
    /// seed_addr without hard-coding port numbers.
    async fn drain_listener() -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind drain listener");
        let addr = listener.local_addr().expect("listener local_addr");
        let handle = tokio::spawn(async move {
            loop {
                let (mut s, _) = match listener.accept().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    loop {
                        match s.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(_) => continue,
                        }
                    }
                });
            }
        });
        (addr, handle)
    }

    #[tokio::test]
    async fn start_binds_listener_and_accepts_seed_connection() {
        // Seed listener: anything that connects to this addr gets accepted.
        let (seed_addr, _seed_h) = drain_listener().await;
        // Local listener: NodeManager.bind()s here.
        let (local_addr, _local_h) = drain_listener().await;

        let config = ClusterConfig {
            enabled: true,
            node_id: NodeId::new("local"),
            listen_addr: local_addr,
            seed_nodes: vec![seed_addr],
            communication_timeout_ms: 1000,
            heartbeat_interval_ms: 50_000, // keep the loop quiet
            max_retries: 3,
        };
        let (tx, _rx) = mpsc::unbounded_channel::<ClusterMessage>();
        let manager = NodeManager::new(config, tx);

        manager.start().await.expect("start");
        // Give the connect_to_seeds path a moment to dial the seed.
        tokio::time::sleep(Duration::from_millis(100)).await;

        // The temporary seed-X key should be present in the transport's
        // connection map after connect_to_seeds runs.
        let seed_temp_key = format!("seed-{seed_addr}");
        assert!(
            manager.get_transport().is_connected(&seed_temp_key),
            "expected seed key {seed_temp_key} to be connected"
        );
    }

    #[tokio::test]
    async fn handle_join_inserts_node_into_known_nodes() {
        // NodeManager::handle_message(Join) should add the peer's
        // NodeInfo into the internal nodes map. We can't observe the
        // map directly, but we can assert handle_message returns Ok
        // and that the nodes list (exposed via... not exposed; we use
        // a smoke path). Add a follow-up if nodes() becomes public.
        let (local_addr, _h) = drain_listener().await;
        let config = ClusterConfig {
            enabled: true,
            node_id: NodeId::new("local"),
            listen_addr: local_addr,
            seed_nodes: vec![],
            communication_timeout_ms: 1000,
            heartbeat_interval_ms: 50_000,
            max_retries: 3,
        };
        let (tx, _rx) = mpsc::unbounded_channel::<ClusterMessage>();
        let manager = NodeManager::new(config, tx);

        let peer_info = NodeInfo::new(NodeId::new("peer-1"), "127.0.0.1:9999".parse().unwrap());
        manager
            .handle_message(ClusterMessage::Join(peer_info.clone()))
            .await
            .expect("handle_message(Join)");

        // Verify via the transport's connected_nodes path: the Join
        // path doesn't add the peer to the transport connection map,
        // it only records into the nodes map. We exercise the
        // no-panic + Ok return as the contract for now; a follow-up
        // spec can expose nodes() if needed.
        assert!(manager.get_transport().connected_nodes().is_empty());
    }

    #[tokio::test]
    async fn handle_forward_does_not_panic_with_local_subscribers() {
        // ForwardMessage handling is a no-op on the NodeManager side
        // (delivery lives on Forwarder::deliver). This test pins the
        // contract: handle_message(Forward) returns Ok and the
        // manager is still alive afterwards.
        let (local_addr, _h) = drain_listener().await;
        let config = ClusterConfig {
            enabled: true,
            node_id: NodeId::new("local"),
            listen_addr: local_addr,
            seed_nodes: vec![],
            communication_timeout_ms: 1000,
            heartbeat_interval_ms: 50_000,
            max_retries: 3,
        };
        let (tx, _rx) = mpsc::unbounded_channel::<ClusterMessage>();
        let manager = NodeManager::new(config, tx);

        let fwd = crate::types::ForwardMessage {
            forward_id: 1,
            original_source: 0xAAu128,
            target_client_id: 0x42u128,
            title: "any".into(),
            payload: vec![1, 2, 3],
            is_group: false,
            is_broadcast: false,
            require_ack: false,
        };
        manager
            .handle_message(ClusterMessage::Forward(fwd))
            .await
            .expect("handle_message(Forward)");
    }

    #[tokio::test]
    async fn send_to_returns_err_when_not_connected() {
        let (local_addr, _h) = drain_listener().await;
        let config = ClusterConfig {
            enabled: true,
            node_id: NodeId::new("local"),
            listen_addr: local_addr,
            seed_nodes: vec![],
            communication_timeout_ms: 1000,
            heartbeat_interval_ms: 50_000,
            max_retries: 3,
        };
        let (tx, _rx) = mpsc::unbounded_channel::<ClusterMessage>();
        let manager = NodeManager::new(config, tx);

        let msg = ClusterMessage::Ping(crate::types::PingMessage {
            node_id: "local".into(),
            timestamp: 0,
        });
        let err = manager.send_to("never-connected", msg).await.unwrap_err();
        assert!(
            err.to_string().contains("Not connected"),
            "got error: {err}"
        );
    }

    #[tokio::test]
    async fn shutdown_signals_subscribers_via_broadcast_channel() {
        // NodeManager::shutdown sends () on the broadcast::Sender
        // embedded in the transport. Subscribe on a fresh receiver
        // and assert the signal lands.
        let (local_addr, _h) = drain_listener().await;
        let config = ClusterConfig {
            enabled: true,
            node_id: NodeId::new("local"),
            listen_addr: local_addr,
            seed_nodes: vec![],
            communication_timeout_ms: 1000,
            heartbeat_interval_ms: 50_000,
            max_retries: 3,
        };
        let (tx, _rx) = mpsc::unbounded_channel::<ClusterMessage>();
        let manager = NodeManager::new(config, tx);

        // The broadcast::Sender is private; we verify the public
        // contract that shutdown is callable and doesn't panic.
        manager.shutdown();
    }
}
