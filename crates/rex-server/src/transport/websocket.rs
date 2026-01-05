use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use futures_util::StreamExt;
use rex_core::{RexClientInner, utils::new_uuid};
use rex_sender::WebSocketSender;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use tracing::{debug, info, warn};

use super::base::ServerBase;
use crate::{RexServerConfig, RexServerTrait, RexSystem};

pub struct WebSocketServer {
    base: ServerBase,
    listener: Arc<TcpListener>,
}

#[async_trait::async_trait]
impl RexServerTrait for WebSocketServer {
    async fn close(&self) {
        self.base.send_shutdown_signal();
        info!("WebSocketServer shutdown complete");
    }
}

impl WebSocketServer {
    pub async fn open(
        system: Arc<RexSystem>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;
        let listener = TcpListener::bind(addr).await?;

        let (base, mut shutdown_rx) = ServerBase::new(system, config);

        let server = Arc::new(WebSocketServer {
            base,
            listener: Arc::new(listener),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server = server.clone();
            async move {
                info!("Accepting WebSocket connections on {}", addr);
                loop {
                    tokio::select! {
                        Ok((stream, peer_addr)) = server.listener.accept() => {
                            server.clone().handle_connection(stream, peer_addr).await;
                        }
                        _ = shutdown_rx.recv() => {
                            info!("WebSocket server received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
                info!("Stopped accepting WebSocket connections");
            }
        });

        Ok(server)
    }

    async fn handle_connection(self: Arc<Self>, stream: TcpStream, peer_addr: SocketAddr) {
        info!("New WebSocket connection from {}", peer_addr);

        // WebSocket 握手
        let ws_stream = match accept_async(stream).await {
            Ok(ws) => ws,
            Err(e) => {
                warn!("WebSocket handshake failed for {}: {}", peer_addr, e);
                return;
            }
        };

        let (sink, mut stream) = ws_stream.split();
        let sender = Arc::new(WebSocketSender::new_server(sink));
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

                server
                    .handle_connection_inner(peer.clone(), &mut stream)
                    .await;

                let client_id = peer.id();
                server.base.system.remove_client(client_id).await;

                info!("WebSocket connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(
        self: &Arc<Self>,
        peer: Arc<RexClientInner>,
        stream: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<TcpStream>,
        >,
    ) {
        let peer_addr = peer.local_addr();
        info!("Handling WebSocket connection: {}", peer_addr);

        let mut buffer = BytesMut::with_capacity(self.base.config.max_buffer_size);
        let mut shutdown_rx = self.base.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                // 从 WebSocket 流中读取数据
                result = stream.next() => {
                    match result {
                        Some(Ok(Message::Binary(data))) => {
                            // 将读取的数据添加到缓冲区
                            buffer.extend_from_slice(&data);

                            // 使用 ServerBase 的通用解析和处理逻辑
                            if let Err(e) = self.base.parse_and_handle_buffer(&peer, &mut buffer).await {
                                warn!("Error processing buffer for {}: {}", peer_addr, e);
                            }
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("WebSocket connection {} closed by client", peer_addr);
                            break;
                        }
                        Some(Ok(Message::Ping(data))) => {
                            // 自动响应 Pong
                            debug!("Received ping from {}, sending pong", peer_addr);
                            if let Err(e) = peer.send_buf(&Message::Pong(data).into_data()).await {
                                warn!("Failed to send pong to {}: {}", peer_addr, e);
                            }
                        }
                        Some(Ok(_)) => {
                            // 忽略其他类型的消息(Text, Pong等)
                            debug!("Received non-binary message from {}", peer_addr);
                        }
                        Some(Err(e)) => {
                            info!("WebSocket connection {} error: {}", peer_addr, e);
                            break;
                        }
                        None => {
                            info!("WebSocket connection {} stream ended", peer_addr);
                            break;
                        }
                    }
                }
                // 监听关闭信号
                _ = shutdown_rx.recv() => {
                    info!("WebSocket connection {} shutting down due to server shutdown", peer_addr);
                    break;
                }
            }
        }

        info!("Finished handling WebSocket connection: {}", peer_addr);
    }
}
