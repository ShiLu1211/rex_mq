use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use rex_core::{RexClientInner, utils::new_uuid};
use rex_sender::TcpSender;
use tokio::net::{TcpListener, TcpStream, tcp::OwnedReadHalf};
use tracing::{info, warn};

use super::base::ServerBase;
use super::driver::{ConnectionDriver, TcpByteSource};
use crate::{RexServerConfig, RexServerTrait, Services};

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

    fn addr(&self) -> SocketAddr {
        self.base.config.bind_addr
    }

    async fn ready(&self) {
        self.base.wait_ready().await;
    }
}

impl TcpServer {
    pub async fn open(
        services: Arc<Services>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;
        let listener = TcpListener::bind(addr).await?;

        let (base, mut shutdown_rx) = ServerBase::new(services, config);

        let server = Arc::new(TcpServer {
            base,
            listener: Arc::new(listener),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server = server.clone();
            async move {
                info!("Accepting TCP connections on {}", addr);
                server.base.mark_ready();
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
                server.base.services.remove_client(client_id).await;

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

        let driver = ConnectionDriver::new(
            &self.base.services,
            &peer,
            "TCP",
            self.base.config.max_buffer_size,
        );
        let source = TcpByteSource {
            reader: &mut reader,
        };
        driver.drive(source).await;

        info!("Finished handling TCP connection: {}", peer_addr);
    }
}
