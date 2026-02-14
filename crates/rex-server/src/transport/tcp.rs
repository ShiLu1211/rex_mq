use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{RexClientInner, utils::new_uuid};
use rex_sender::TcpSender;
use tokio::{
    io::AsyncReadExt,
    net::{TcpListener, TcpStream, tcp::OwnedReadHalf},
};
use tracing::{debug, info, warn};

use super::base::ServerBase;
use crate::{RexServerConfig, RexServerTrait, RexSystem};

pub struct TcpServer {
    base: ServerBase,
    listener: Arc<TcpListener>,
}

#[async_trait::async_trait]
impl RexServerTrait for TcpServer {
    async fn close(&self) {
        self.base.send_shutdown_signal();
        info!("TcpServer shutdown complete");
    }
}

impl TcpServer {
    pub async fn open(
        system: Arc<RexSystem>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;
        let listener = TcpListener::bind(addr).await?;

        let (base, mut shutdown_rx) = ServerBase::new(system, config);

        let server = Arc::new(TcpServer {
            base,
            listener: Arc::new(listener),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server = server.clone();
            async move {
                info!("Accepting TCP connections on {}", addr);
                loop {
                    tokio::select! {
                        Ok((stream, peer_addr)) = server.listener.accept() => {
                            server.clone().handle_connection(stream, peer_addr).await;
                        }
                        _ = shutdown_rx.recv() => {
                            info!("TCP server received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
                info!("Stopped accepting TCP connections");
            }
        });

        Ok(server)
    }

    async fn handle_connection(self: Arc<Self>, stream: TcpStream, peer_addr: SocketAddr) {
        info!("New TCP connection from {}", peer_addr);

        if let Err(e) = stream.set_nodelay(true) {
            warn!("Error setting TCP_NODELAY for {}: {}", peer_addr, e);
        }

        let (reader, writer) = stream.into_split();
        let sender = Arc::new(TcpSender::new(writer));
        let peer = Arc::new(RexClientInner::new(new_uuid(), peer_addr, "", sender));

        let permit = match self.base.acquire_connection_permit().await {
            Ok(permit) => permit,
            Err(e) => {
                warn!("Too many connections, rejecting {}: {}", peer_addr, e);
                return;
            }
        };

        // 为每个连接启动处理任务
        tokio::spawn({
            let server = self.clone();
            async move {
                let _permit = permit;

                server.handle_connection_inner(peer.clone(), reader).await;

                let client_id = peer.id();
                server.base.system.remove_client(client_id).await;

                info!("TCP connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(
        self: &Arc<Self>,
        peer: Arc<RexClientInner>,
        mut reader: OwnedReadHalf,
    ) {
        let peer_addr = peer.local_addr();
        info!("Handling TCP connection: {}", peer_addr);

        let mut buffer = BytesMut::with_capacity(self.base.config.max_buffer_size);
        let mut shutdown_rx = self.base.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                // 从 TCP 流中读取数据
                result = reader.read_buf(&mut buffer) => {
                    match result {
                        Ok(0) => {
                            info!("TCP connection {} closed by client", peer_addr);
                            break;
                        }
                        Ok(_) => {
                            debug!("TCP connection {} received {} bytes", peer_addr, buffer.len());
                            if let Err(e) = self.base.parse_and_handle_buffer(&peer, &mut buffer).await {
                                warn!("Error processing buffer for {}: {}", peer_addr, e);
                            }
                        }
                        Err(e) => {
                            info!("TCP connection {} read error: {}", peer_addr, e);
                            break;
                        }
                    }
                }
                // 监听关闭信号
                _ = shutdown_rx.recv() => {
                    info!("TCP connection {} shutting down due to server shutdown", peer_addr);
                    break;
                }
            }
        }

        info!("Finished handling TCP connection: {}", peer_addr);
    }
}
