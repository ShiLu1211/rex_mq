use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use rex_client::{RexClientInner, TcpSender};
use rex_core::{RexData, utils::new_uuid};
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream, tcp::OwnedReadHalf},
    sync::{Semaphore, broadcast},
};
use tracing::{debug, info, warn};

use crate::{RexServerConfig, RexServerTrait, RexSystem, handler::handle};

pub struct TcpServer {
    system: Arc<RexSystem>,
    config: RexServerConfig,
    listener: Arc<TcpListener>,
    semaphore: Arc<Semaphore>,
    shutdown_tx: Arc<broadcast::Sender<()>>,
}
#[async_trait::async_trait]
impl RexServerTrait for TcpServer {
    async fn close(&self) {
        // Send shutdown signal to all tasks
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Error sending shutdown signal: {}", e);
        }

        info!("Shutdown complete");
    }
}

impl TcpServer {
    pub async fn open(
        system: Arc<RexSystem>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;
        let listener = TcpListener::bind(addr).await?;
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_handlers));

        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1 + config.max_concurrent_handlers);
        let server = Arc::new(TcpServer {
            system,
            config,
            listener: Arc::new(listener),
            semaphore,
            shutdown_tx: Arc::new(shutdown_tx),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server_ = server.clone();
            async move {
                info!("Accepting connections on {}", addr);
                loop {
                    tokio::select! {
                        Ok((stream, peer_addr)) =  server_.listener.accept() => {
                            server_.clone().handle_connection(stream, peer_addr).await
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

        Ok(server)
    }

    async fn handle_connection(self: Arc<Self>, stream: TcpStream, peer_addr: SocketAddr) {
        info!("New connection from {}", peer_addr);

        if let Err(e) = stream.set_nodelay(true) {
            warn!("Error setting TCP_NODELAY for {}: {}", peer_addr, e);
        }

        let (reader, writer) = stream.into_split();
        let sender = Arc::new(TcpSender::new(writer));
        let peer = Arc::new(RexClientInner::new(new_uuid(), peer_addr, "", sender));

        let permit = match self.semaphore.clone().acquire_owned().await {
            Ok(permit) => permit,
            Err(e) => {
                warn!("Too many connections, rejecting {}, {}", peer_addr, e);
                return;
            }
        };
        // 为每个连接启动处理任务
        tokio::spawn({
            let server_clone = self.clone();
            async move {
                let _permit = permit;

                server_clone
                    .handle_connection_inner(peer.clone(), reader)
                    .await;

                let client_id = peer.id();
                server_clone.system.remove_client(client_id).await;

                info!("Connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(
        self: &Arc<Self>,
        peer: Arc<RexClientInner>,
        mut reader: OwnedReadHalf,
    ) {
        let peer_addr = peer.local_addr();
        info!("Handling new connection: {}", peer_addr);

        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut temp_buf = vec![0u8; self.config.read_buffer_size];

        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                // 从 TCP 流中读取数据
                result = reader.read(&mut temp_buf) => {
                    match result {
                        Ok(0) => {
                            info!("Connection {} closed by client", peer_addr);
                            break;
                        }
                        Ok(n) => {
                            // 将读取的数据添加到缓冲区
                            buffer.extend_from_slice(&temp_buf[..n]);

                            loop {
                                match RexData::try_deserialize(&mut buffer) {
                                    Ok(Some(mut data)) => {
                                        debug!(
                                            "Received data from {}: command={:?}",
                                            peer_addr,
                                            data.header().command(),
                                        );

                                        if let Err(e) = handle(&self.system, &peer, &mut data).await {
                                            warn!("Error handling data from {}: {}", peer_addr, e);
                                        }

                                        peer.update_last_recv();
                                    }
                                    Ok(None) => {
                                        break;
                                    }
                                    Err(e) => {
                                        warn!(
                                            "Error parsing data from {}: {}, clearing buffer",
                                            peer_addr, e
                                        );
                                        buffer.clear();
                                        break;
                                    }
                                }
                            }

                            // 检查缓冲区大小，防止内存泄漏
                            if buffer.len() > self.config.max_buffer_size {
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
                // 监听关闭信号
                _ = shutdown_rx.recv() => {
                    info!("Connection {} shutting down due to server shutdown", peer_addr);
                    break;
                }
            }
        }
    }
}
