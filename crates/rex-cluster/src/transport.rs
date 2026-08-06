//! Cluster Transport Layer
//!
//! Provides TCP-based communication between cluster nodes

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Result;
use bytes::{Buf, BufMut, Bytes, BytesMut};
use dashmap::DashMap;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use crate::types::{ClusterMessage, NodeId};

/// Maximum message size (16MB)
const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Header size: 4 bytes (message length)
const HEADER_SIZE: usize = 4;

/// Cluster transport for inter-node communication
pub struct ClusterTransport {
    /// Local node ID
    local_node_id: NodeId,
    /// Active connections to other nodes (node_id -> sender channel)
    /// Wrapped in Arc so it can be shared across cloned transports
    connections: Arc<DashMap<String, mpsc::Sender<Bytes>>>,
    /// Channel for incoming messages
    message_tx: mpsc::UnboundedSender<IncomingMessage>,
    /// Shutdown signal
    shutdown_tx: broadcast::Sender<()>,
    /// Write tasks handle map
    write_handles: DashMap<String, tokio::task::JoinHandle<()>>,
}

impl Clone for ClusterTransport {
    fn clone(&self) -> Self {
        Self {
            local_node_id: self.local_node_id.clone(),
            connections: Arc::clone(&self.connections), // Share the same connections map
            message_tx: self.message_tx.clone(),
            shutdown_tx: self.shutdown_tx.clone(),
            write_handles: DashMap::new(), // Don't clone write handles
        }
    }
}

/// Incoming message from a remote node
#[derive(Debug)]
pub struct IncomingMessage {
    /// Source node ID
    pub source_node: NodeId,
    /// The message
    pub message: ClusterMessage,
}

impl ClusterTransport {
    /// Create a new cluster transport
    pub fn new(local_node_id: NodeId, message_tx: mpsc::UnboundedSender<IncomingMessage>) -> Self {
        let (shutdown_tx, _) = broadcast::channel(1);
        Self {
            local_node_id,
            connections: Arc::new(DashMap::new()),
            message_tx,
            shutdown_tx,
            write_handles: DashMap::new(),
        }
    }

    /// Start listening for incoming connections
    pub async fn start_listener(&self, listen_addr: SocketAddr) -> Result<()> {
        let listener = TcpListener::bind(listen_addr).await?;
        info!("Cluster transport listening on {}", listen_addr);

        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                result = listener.accept() => {
                    match result {
                        Ok((stream, addr)) => {
                            let this = self.clone();
                            let message_tx = self.message_tx.clone();
                            let shutdown = self.shutdown_tx.clone();

                            tokio::spawn(async move {
                                if let Err(e) = Self::handle_incoming_connection(
                                    stream, addr, message_tx, shutdown, this
                                ).await {
                                    error!("Error handling incoming connection: {}", e);
                                }
                            });
                        }
                        Err(e) => {
                            error!("Failed to accept connection: {}", e);
                        }
                    }
                }
                _ = shutdown_rx.recv() => {
                    info!("Cluster transport shutting down");
                    break;
                }
            }
        }

        Ok(())
    }

    /// Handle an incoming connection
    async fn handle_incoming_connection(
        mut stream: TcpStream,
        _addr: SocketAddr,
        message_tx: mpsc::UnboundedSender<IncomingMessage>,
        shutdown: broadcast::Sender<()>,
        transport: ClusterTransport,
    ) -> Result<()> {
        let mut buffer = BytesMut::new();
        let mut shutdown_rx = shutdown.subscribe();
        let _local_node_id = transport.local_node_id.clone();

        info!(
            "handle_incoming_connection started for local node {}",
            _local_node_id
        );

        // For tracking the connection for sending responses back
        let (write_tx, mut write_rx) = mpsc::channel::<Bytes>(100);

        // First, wait for the Join message to know the node_id
        let mut node_id_str: Option<String> = None;
        let mut stored_connection = false;

        loop {
            tokio::select! {
                result = stream.read_buf(&mut buffer) => {
                    let n = result?;
                    if n == 0 {
                        // Connection closed
                        debug!("Connection closed");
                        break;
                    }
                    debug!("Read {} bytes from connection", n);
                }
                _ = shutdown_rx.recv() => {
                    info!("Shutdown received, breaking loop");
                    break;
                }
                Some(data) = write_rx.recv() => {
                    info!("Got data to write: {} bytes", data.len());
                    debug!("Writing {} bytes to connection", data.len());
                    // Forward received data to write to the stream
                    if let Err(e) = stream.write_all(&data).await {
                        warn!("Failed to write to connection: {}", e);
                        break;
                    }
                    debug!("Wrote {} bytes to connection", data.len());
                }
            }
            // Debug: show if we're still in the loop
            debug!("Select loop iteration done");

            // Parse messages from buffer
            while buffer.len() >= HEADER_SIZE {
                // Get length from first 4 bytes (big-endian)
                let length = ((buffer[0] as usize) << 24)
                    | ((buffer[1] as usize) << 16)
                    | ((buffer[2] as usize) << 8)
                    | (buffer[3] as usize);

                if length > MAX_MESSAGE_SIZE {
                    error!("Message too large: {} bytes", length);
                    // Skip this message by consuming the header
                    buffer.advance(HEADER_SIZE);
                    continue;
                }

                // Check if we have the full message
                let total_needed = HEADER_SIZE + length;
                if buffer.len() < total_needed {
                    // Not enough data, wait for more
                    break;
                }

                // Extract message data
                buffer.advance(HEADER_SIZE);
                let data = buffer.split_to(length);

                // Deserialize and handle message
                match bincode::deserialize::<ClusterMessage>(&data) {
                    Ok(message) => {
                        // For incoming connections, we need to get the node ID from Join message
                        // and store the connection for sending responses back
                        if !stored_connection && let ClusterMessage::Join(node_info) = &message {
                            let nid = node_info.node_id.as_str().to_string();
                            info!(
                                "Storing connection with key '{}' in transport for {}",
                                nid,
                                transport.local_node_id.as_str()
                            );
                            transport.connections.insert(nid.clone(), write_tx.clone());
                            info!(
                                "Connections map now has: {:?}",
                                transport
                                    .connections
                                    .iter()
                                    .map(|e| e.key().clone())
                                    .collect::<Vec<_>>()
                            );
                            node_id_str = Some(nid);
                            stored_connection = true;
                        }

                        let source_node = match &message {
                            ClusterMessage::Join(node_info) => node_info.node_id.clone(),
                            _ => node_id_str
                                .as_ref()
                                .map(|s| NodeId::new(s.as_str()))
                                .unwrap_or_else(|| NodeId::new("unknown")),
                        };

                        if let Err(e) = message_tx.send(IncomingMessage {
                            source_node,
                            message,
                        }) {
                            error!("Failed to send message to handler: {}", e);
                        }
                    }
                    Err(e) => {
                        error!("Failed to deserialize message: {}", e);
                    }
                }
            }
        }

        Ok(())
    }

    /// Connect to a remote node
    pub async fn connect(&self, node_id: NodeId, addr: SocketAddr) -> Result<()> {
        let node_id_str = node_id.as_str().to_string();

        // Check if already connected
        if self.connections.contains_key(&node_id_str) {
            debug!("Already connected to node {}", node_id_str);
            return Ok(());
        }

        info!("Connecting to cluster node {} at {}", node_id_str, addr);

        let stream = TcpStream::connect(addr).await?;
        stream.set_nodelay(true)?;

        // Split the stream into read and write halves
        let (reader, writer) = tokio::io::split(stream);

        let (sender, receiver) = mpsc::channel::<Bytes>(100);

        // Insert sender before starting write task
        self.connections.insert(node_id_str.clone(), sender);

        // Spawn writer task
        let node_id_clone = node_id_str.clone();
        let write_handle = tokio::spawn(async move {
            Self::write_loop_with_sender(node_id_clone, writer, receiver).await;
        });

        // Spawn read task to handle incoming messages
        let node_id_read = node_id_str.clone();
        let message_tx_clone = self.message_tx.clone();
        let local_node_id_for_read = self.local_node_id.clone();
        tokio::spawn(async move {
            Self::read_loop_with_receiver(
                node_id_read,
                reader,
                message_tx_clone,
                local_node_id_for_read,
            )
            .await;
        });

        self.write_handles.insert(node_id_str.clone(), write_handle);

        Ok(())
    }

    /// Write loop for a connection
    async fn write_loop_with_sender(
        node_id: String,
        mut writer: tokio::io::WriteHalf<TcpStream>,
        mut receiver: mpsc::Receiver<Bytes>,
    ) {
        info!("Write loop started for {}", node_id);
        while let Some(data) = receiver.recv().await {
            info!(
                "Write loop for {}: got {} bytes to send",
                node_id,
                data.len()
            );
            if let Err(e) = writer.write_all(&data).await {
                error!("Failed to write to {}: {}", node_id, e);
                break;
            }
            if let Err(e) = writer.flush().await {
                error!("Failed to flush {}: {}", node_id, e);
                break;
            }
            info!("Write loop for {}: sent {} bytes", node_id, data.len());
        }
        debug!("Write loop exited for {}", node_id);
    }

    /// Read loop for a connection - handles incoming messages
    async fn read_loop_with_receiver(
        node_id: String,
        mut reader: tokio::io::ReadHalf<TcpStream>,
        message_tx: mpsc::UnboundedSender<IncomingMessage>,
        _local_node_id: NodeId,
    ) {
        info!("Read loop started for {}", node_id);
        let mut buffer = BytesMut::new();

        loop {
            match reader.read_buf(&mut buffer).await {
                Ok(0) => {
                    debug!("Read loop: connection closed for {}", node_id);
                    break;
                }
                Ok(n) => {
                    debug!(
                        "Read loop: read {} bytes from {}, buffer len: {}, first 8 bytes: {:02x?}",
                        n,
                        node_id,
                        buffer.len(),
                        &buffer[..buffer.len().min(8)]
                    );
                    // Parse messages from buffer
                    while buffer.len() >= HEADER_SIZE {
                        // Get length from first 4 bytes (big-endian)
                        let length = ((buffer[0] as usize) << 24)
                            | ((buffer[1] as usize) << 16)
                            | ((buffer[2] as usize) << 8)
                            | (buffer[3] as usize);

                        if length > MAX_MESSAGE_SIZE {
                            error!("Message too large: {} bytes", length);
                            // Skip this message by consuming the header
                            buffer.advance(HEADER_SIZE);
                            continue;
                        }

                        // Check if we have the full message (HEADER_SIZE bytes header + length bytes payload)
                        let total_needed = HEADER_SIZE + length;
                        if buffer.len() < total_needed {
                            // Not enough data, wait for more
                            break;
                        }

                        // We have enough data, consume header and extract message
                        buffer.advance(HEADER_SIZE);
                        let data = buffer.split_to(length);

                        match bincode::deserialize::<ClusterMessage>(&data) {
                            Ok(message) => {
                                info!(
                                    "Read loop: successfully deserialized message {:?} from {}",
                                    message, node_id
                                );
                                if let Err(e) = message_tx.send(IncomingMessage {
                                    source_node: NodeId::new(node_id.as_str()),
                                    message,
                                }) {
                                    error!("Failed to send message to handler: {}", e);
                                }
                            }
                            Err(e) => {
                                error!(
                                    "Failed to deserialize message (data len {}): {}",
                                    data.len(),
                                    e
                                );
                            }
                        }
                    }
                }
                Err(e) => {
                    error!("Read error from {}: {}", node_id, e);
                    break;
                }
            }
        }
        debug!("Read loop exited for {}", node_id);
    }

    /// Send a message to a specific node
    pub async fn send_to(&self, node_id: &str, message: &ClusterMessage) -> Result<()> {
        // Find connection
        let sender = self
            .connections
            .get(node_id)
            .ok_or_else(|| anyhow::anyhow!("Not connected to node {}", node_id))?;

        // Serialize message
        let data = bincode::serialize(message)?;
        let length = data.len() as u32;

        // Create frame: [length:4][data]
        let mut frame = BytesMut::with_capacity(HEADER_SIZE + data.len());
        frame.put_u32(length);
        frame.extend_from_slice(&data);

        debug!("Sending frame to {} ({} bytes)", node_id, length);
        if let Err(e) = sender.send(frame.freeze()).await {
            error!("Failed to send to {}: {}", node_id, e);
            return Err(e.into());
        }
        debug!("Sent frame to {}", node_id);

        Ok(())
    }

    /// Broadcast a message to all connected nodes
    pub async fn broadcast(&self, message: &ClusterMessage) -> Result<()> {
        let node_ids: Vec<String> = self.connections.iter().map(|e| e.key().clone()).collect();

        for node_id in node_ids {
            if let Err(e) = self.send_to(&node_id, message).await {
                warn!("Failed to broadcast to {}: {}", node_id, e);
            }
        }

        Ok(())
    }

    /// Check if connected to a node
    pub fn is_connected(&self, node_id: &str) -> bool {
        self.connections.contains_key(node_id)
    }

    /// Remove a connection to a node
    pub fn remove_connection(&self, node_id: &str) {
        if self.connections.remove(node_id).is_some() {
            debug!("Removed connection to node {}", node_id);
        }
        // Also remove the write handle if exists
        if let Some((_, handle)) = self.write_handles.remove(node_id) {
            handle.abort();
        }
    }

    /// Get all connected node IDs
    pub fn connected_nodes(&self) -> Vec<String> {
        self.connections.iter().map(|e| e.key().clone()).collect()
    }

    /// Shutdown the transport
    pub fn shutdown(&self) {
        let _ = self.shutdown_tx.send(());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[tokio::test]
    async fn test_transport_creation() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("test-node"), tx);
        assert!(transport.connected_nodes().is_empty());
    }

    // ---------- New tests (PR 2: rex-cluster test coverage) ----------

    /// Bind an ephemeral TCP listener on 127.0.0.1:0 that accepts
    /// connections and drains incoming bytes until the peer closes.
    /// Returns the bound address plus the accept-loop JoinHandle so
    /// tests can assert clean shutdown.
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

    /// Same as `drain_listener` but echoes each framed payload
    /// (4-byte big-endian length header already stripped) into the
    /// returned `mpsc::Receiver<Vec<u8>>` so tests can assert on
    /// the bytes that hit the wire.
    async fn recording_listener() -> (
        SocketAddr,
        tokio::task::JoinHandle<()>,
        mpsc::UnboundedReceiver<Vec<u8>>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind recording listener");
        let addr = listener.local_addr().expect("listener local_addr");
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let handle = tokio::spawn(async move {
            loop {
                let (mut s, _) = match listener.accept().await {
                    Ok(p) => p,
                    Err(_) => break,
                };
                let frame_tx = frame_tx.clone();
                tokio::spawn(async move {
                    loop {
                        let mut length = [0u8; 4];
                        if s.read_exact(&mut length).await.is_err() {
                            break;
                        }
                        let mut payload = vec![0u8; u32::from_be_bytes(length) as usize];
                        if s.read_exact(&mut payload).await.is_err() {
                            break;
                        }
                        let _ = frame_tx.send(payload);
                    }
                });
            }
        });
        (addr, handle, frame_rx)
    }

    #[tokio::test]
    async fn connect_then_is_connected_returns_true() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("local"), tx);
        let (addr, _h) = drain_listener().await;

        transport
            .connect(NodeId::new("peer"), addr)
            .await
            .expect("connect");

        assert!(transport.is_connected("peer"));
        assert_eq!(transport.connected_nodes(), vec!["peer".to_string()]);
    }

    #[tokio::test]
    async fn remove_connection_drops_peer_from_connected_nodes() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("local"), tx);
        let (addr, _h) = drain_listener().await;

        transport.connect(NodeId::new("peer"), addr).await.unwrap();
        assert!(transport.is_connected("peer"));

        transport.remove_connection("peer");
        assert!(!transport.is_connected("peer"));
        assert!(transport.connected_nodes().is_empty());
    }

    #[tokio::test]
    async fn send_to_unknown_node_returns_err() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("local"), tx);
        let msg = ClusterMessage::Ping(crate::types::PingMessage {
            node_id: "local".into(),
            timestamp: 0,
        });
        let err = transport
            .send_to("never-connected", &msg)
            .await
            .unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("Not connected"), "got error: {msg}");
    }

    #[tokio::test]
    async fn broadcast_with_no_connections_returns_ok_with_zero_sends() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("local"), tx);
        let msg = ClusterMessage::Ping(crate::types::PingMessage {
            node_id: "local".into(),
            timestamp: 0,
        });
        // Empty connection set; broadcast should be a no-op Ok.
        transport
            .broadcast(&msg)
            .await
            .expect("broadcast with 0 peers");
    }

    #[tokio::test]
    async fn send_to_round_trips_a_framed_message() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("local"), tx);
        let (addr, _h, mut received) = recording_listener().await;
        transport.connect(NodeId::new("peer"), addr).await.unwrap();

        let original = ClusterMessage::Ping(crate::types::PingMessage {
            node_id: "local".into(),
            timestamp: 42,
        });
        transport.send_to("peer", &original).await.expect("send_to");

        // Drain the framed payload (length header already stripped).
        let payload = tokio::time::timeout(Duration::from_secs(1), received.recv())
            .await
            .expect("timed out waiting for frame")
            .expect("listener closed");
        let decoded: ClusterMessage = bincode::deserialize(&payload).expect("bincode deserialize");
        match decoded {
            ClusterMessage::Ping(p) => {
                assert_eq!(p.node_id, "local");
                assert_eq!(p.timestamp, 42);
            }
            other => panic!("expected Ping, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn broadcast_fan_outs_to_all_connected_peers() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let transport = ClusterTransport::new(NodeId::new("local"), tx);
        let (addr_a, _h_a, mut rx_a) = recording_listener().await;
        let (addr_b, _h_b, mut rx_b) = recording_listener().await;
        transport.connect(NodeId::new("a"), addr_a).await.unwrap();
        transport.connect(NodeId::new("b"), addr_b).await.unwrap();

        let msg = ClusterMessage::Ping(crate::types::PingMessage {
            node_id: "local".into(),
            timestamp: 7,
        });
        transport.broadcast(&msg).await.expect("broadcast");

        for (peer, rx) in [("a", &mut rx_a), ("b", &mut rx_b)] {
            let payload = tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {peer} frame"))
                .unwrap_or_else(|| panic!("{peer} listener closed"));
            let decoded: ClusterMessage =
                bincode::deserialize(&payload).expect("bincode deserialize");
            match decoded {
                ClusterMessage::Ping(p) => assert_eq!(p.timestamp, 7),
                other => panic!("{peer} expected Ping, got {other:?}"),
            }
        }
    }
}
