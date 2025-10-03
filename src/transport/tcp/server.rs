use std::{net::SocketAddr, sync::Arc, time::Duration};

use anyhow::Result;
use bytes::{Buf, BytesMut};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream, tcp::OwnedReadHalf},
    sync::broadcast,
};
use tracing::{debug, error, info, warn};

use crate::{
    RexClientInner, RexServer, RexServerConfig, RexSystem, TcpSender,
    handler::handle,
    protocol::RexData,
    utils::{new_uuid, now_secs},
};

pub struct TcpServer {
    system: Arc<RexSystem>,
    config: RexServerConfig,
    listener: Arc<TcpListener>,
    shutdown_tx: Arc<broadcast::Sender<()>>,
}
#[async_trait::async_trait]
impl RexServer for TcpServer {
    async fn open(system: Arc<RexSystem>, config: RexServerConfig) -> Result<Arc<Self>> {
        let addr = config.bind_addr;
        let listener = TcpListener::bind(addr).await?;

        let (shutdown_tx, _) = broadcast::channel(4);
        let server = Arc::new(TcpServer {
            system,
            config,
            listener: Arc::new(listener),
            shutdown_tx: Arc::new(shutdown_tx),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server_ = server.clone();
            let mut shutdown_rx = server_.shutdown_tx.subscribe();
            async move {
                info!("Accepting connections on {}", addr);
                loop {
                    tokio::select! {
                        Ok((stream, peer_addr)) =  server_.listener.accept() => {
                            server_.clone().handle_connection(stream, peer_addr).await;
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Server received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
                info!("Stopped accepting connections");
            }
        });

        // 客户端超时清理任务
        tokio::spawn({
            let server_clone = server.clone();
            let mut shutdown_rx = server_clone.shutdown_tx.subscribe();
            async move {
                let check_interval = Duration::from_secs(server_clone.config.check_interval); // 检查频率
                let client_timeout = server_clone.config.client_timeout; // 客户端超时时间（秒）

                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(check_interval) => {
                            server_clone.cleanup_inactive_clients(client_timeout);
                        }
                        _ = shutdown_rx.recv() => {
                            info!("Cleanup task received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
            }
        });

        Ok(server)
    }

    async fn close(&self) {
        // Send shutdown signal to all tasks
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Error sending shutdown signal: {}", e);
        }

        info!("Shutdown complete");
    }
}

impl TcpServer {
    async fn handle_connection(self: Arc<Self>, stream: TcpStream, peer_addr: SocketAddr) {
        info!("New connection from {}", peer_addr);

        if let Err(e) = stream.set_nodelay(true) {
            warn!("Error setting TCP_NODELAY for {}: {}", peer_addr, e);
        }

        let (reader, writer) = stream.into_split();
        let sender = Arc::new(TcpSender::new(writer));
        let peer = Arc::new(RexClientInner::new(new_uuid(), peer_addr, "", sender));

        // 为每个连接启动处理任务
        tokio::spawn({
            let server_clone = self.clone();
            let peer_clone = peer.clone();
            async move {
                server_clone
                    .handle_connection_inner(peer_clone.clone(), reader)
                    .await;
                // 连接断开时清理客户端
                self.remove_client(peer_clone.id());
                info!("Connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(
        self: Arc<Self>,
        peer: Arc<RexClientInner>,
        mut reader: OwnedReadHalf,
    ) {
        let peer_addr = peer.local_addr();
        info!("Handling new connection: {}", peer_addr);

        let mut buffer = BytesMut::new();
        let mut temp_buf = vec![0u8; 8192];

        loop {
            // 从 TCP 流中读取数据
            match reader.read(&mut temp_buf).await {
                Ok(0) => {
                    info!("Connection {} closed by client", peer_addr);
                    break;
                }
                Ok(n) => {
                    // 将读取的数据添加到缓冲区
                    buffer.extend_from_slice(&temp_buf[..n]);

                    // 尝试解析完整的数据包
                    while let Some(parse_result) = RexData::try_deserialize(&buffer) {
                        match parse_result {
                            Ok((mut data, consumed_bytes)) => {
                                debug!(
                                    "Received data from {}: command={:?}, consumed {} bytes",
                                    peer_addr,
                                    data.header().command(),
                                    consumed_bytes
                                );

                                // 移除已消耗的字节
                                buffer.advance(consumed_bytes);

                                // 异步处理数据
                                tokio::spawn({
                                    let peer_clone = peer.clone();
                                    let server_clone = self.clone();
                                    async move {
                                        if let Err(e) =
                                            handle(&server_clone.system, &peer_clone, &mut data)
                                                .await
                                        {
                                            warn!("Error handling data from {}: {}", peer_addr, e);
                                        }
                                    }
                                });
                            }
                            Err(e) => {
                                error!(
                                    "Error parsing data from {}: {}, clearing buffer",
                                    peer_addr, e
                                );
                                buffer.clear();
                                break;
                            }
                        }
                    }

                    // 检查缓冲区大小，防止内存泄漏
                    if buffer.len() > 64 * 1024 {
                        warn!("Buffer too large for connection {}, clearing", peer_addr);
                        buffer.clear();
                    }
                }
                Err(e) => {
                    info!("Connection {} read error: {}", peer_addr, e);
                    break;
                }
            }
        }
    }

    // 移除客户端
    fn remove_client(&self, client_id: u128) {
        let mut clients = self.system.find_all();
        let initial_len = clients.len();
        clients.retain(|client| client.id() != client_id);

        if clients.len() < initial_len {
            info!(
                "Removed client {}, remaining clients: {}",
                client_id,
                clients.len()
            );
        }
    }

    // 清理不活跃的客户端
    fn cleanup_inactive_clients(&self, timeout_secs: u64) {
        let mut clients = self.system.find_all();
        let now = now_secs();
        let initial_count = clients.len();

        clients.retain(|client| {
            let last_active = client.last_recv();
            if now - last_active > timeout_secs {
                warn!(
                    "Client {} (addr: {}) timed out, removing...",
                    client.id(),
                    client.local_addr()
                );
                false
            } else {
                true
            }
        });

        let removed_count = initial_count - clients.len();
        if removed_count > 0 {
            info!("Cleaned up {} inactive clients", removed_count);
        }
    }
}
