use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use futures_util::StreamExt;
use rex_core::{RexClientInner, utils::new_uuid};
use rex_sender::WebSocketSender;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::accept_async;
use tracing::{info, warn};

use super::base::ServerBase;
use super::driver::{ConnectionDriver, WebSocketByteSource};
use crate::{RexServerConfig, RexServerTrait, Services};

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

    fn addr(&self) -> SocketAddr {
        self.base.config.bind_addr
    }

    async fn ready(&self) {
        self.base.wait_ready().await;
    }
}

impl WebSocketServer {
    pub async fn open(
        services: Arc<Services>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;
        let listener = TcpListener::bind(addr).await?;

        let (base, mut shutdown_rx) = ServerBase::new(services, config);

        let server = Arc::new(WebSocketServer {
            base,
            listener: Arc::new(listener),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server = server.clone();
            async move {
                info!("Accepting WebSocket connections on {}", addr);
                server.base.mark_ready();
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
        peer.set_transport_label("websocket");

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
                server.base.services.remove_client(client_id).await;

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

        let driver = ConnectionDriver::new(
            &self.base.services,
            &peer,
            "WebSocket",
            self.base.config.max_buffer_size,
        );
        let source = WebSocketByteSource {
            stream,
            peer: &peer,
        };
        driver.drive(source).await;

        info!("Finished handling WebSocket connection: {}", peer_addr);
    }
}
