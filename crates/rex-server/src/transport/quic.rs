use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::{Bytes, BytesMut};
use quinn::{Connection, Endpoint, RecvStream, ServerConfig};
use rex_core::{RexClientInner, utils::new_uuid};
use rex_sender::QuicSender;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::io::AsyncReadExt;
use tracing::{info, warn};

use super::base::ServerBase;
use super::driver::{ByteSource, ConnectionDriver};
use crate::{RexServerConfig, RexServerTrait, Services};

pub struct QuicServer {
    base: ServerBase,
    endpoint: Endpoint,
}

#[async_trait::async_trait]
impl RexServerTrait for QuicServer {
    async fn close(&self) {
        self.base.send_shutdown_signal();

        // Close endpoint
        self.endpoint.close(0u32.into(), b"server shutdown");

        // Wait a bit for graceful shutdown
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.endpoint.wait_idle().await;

        info!("QuicServer shutdown complete");
    }

    fn addr(&self) -> SocketAddr {
        self.base.config.bind_addr
    }

    async fn ready(&self) {
        self.base.wait_ready().await;
    }
}

impl QuicServer {
    pub async fn open(
        services: Arc<Services>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;

        // 生成自签名证书
        let (cert, key) = generate_self_signed_cert()?;

        // 配置服务器
        let server_config = ServerConfig::with_single_cert(vec![cert], key)?;

        // 创建 endpoint
        let endpoint = Endpoint::server(server_config, addr)?;

        let (base, mut shutdown_rx) = ServerBase::new(services, config);

        let server = Arc::new(QuicServer { base, endpoint });

        // 服务器连接处理任务
        tokio::spawn({
            let server = server.clone();
            async move {
                info!("Accepting QUIC connections on {}", addr);
                server.base.mark_ready();
                loop {
                    tokio::select! {
                        Some(incoming) = server.endpoint.accept() => {
                            let server_clone = server.clone();
                            tokio::spawn(async move {
                                match incoming.await {
                                    Ok(connection) => {
                                        let remote_addr = connection.remote_address();
                                        info!("New QUIC connection from {}", remote_addr);
                                        server_clone.handle_connection(connection, remote_addr).await;
                                    }
                                    Err(e) => {
                                        warn!("Failed to establish QUIC connection: {}", e);
                                    }
                                }
                            });
                        }
                        _ = shutdown_rx.recv() => {
                            info!("QUIC server received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
                info!("Stopped accepting QUIC connections");
            }
        });

        Ok(server)
    }

    async fn handle_connection(self: Arc<Self>, connection: Connection, peer_addr: SocketAddr) {
        info!("Handling QUIC connection from {}", peer_addr);

        // 打开第一个单向流用于发送
        let sender = match connection.open_uni().await {
            Ok(stream) => Arc::new(QuicSender::new(stream)),
            Err(e) => {
                warn!("Failed to open initial stream for {}: {}", peer_addr, e);
                return;
            }
        };

        let peer = Arc::new(RexClientInner::new(new_uuid(), peer_addr, "", sender));
        peer.set_transport_label("quic");

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
                    .handle_connection_inner(peer.clone(), connection)
                    .await;

                let client_id = peer.id();
                server.base.services.remove_client(client_id).await;

                info!("QUIC connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(
        self: &Arc<Self>,
        peer: Arc<RexClientInner>,
        connection: Connection,
    ) {
        let peer_addr = peer.local_addr();
        info!("Processing streams from QUIC connection: {}", peer_addr);

        // 处理所有传入的单向流
        loop {
            match connection.accept_uni().await {
                Ok(recv_stream) => {
                    let peer_clone = peer.clone();
                    let server_clone = self.clone();

                    tokio::spawn(async move {
                        if let Err(e) = server_clone.handle_stream(peer_clone, recv_stream).await {
                            warn!("Error handling stream from {}: {}", peer_addr, e);
                        }
                    });
                }
                Err(e) => {
                    info!("QUIC connection {} closed: {}", peer_addr, e);
                    break;
                }
            }
        }

        info!("Finished processing streams from {}", peer_addr);
    }

    async fn handle_stream(
        self: &Arc<Self>,
        peer: Arc<RexClientInner>,
        recv_stream: RecvStream,
    ) -> Result<()> {
        let driver = ConnectionDriver::new(
            &self.base.services,
            &peer,
            "QUIC",
            self.base.config.max_buffer_size,
        );
        let source = QuicByteSource { recv_stream };
        driver.drive(source).await;
        Ok(())
    }
}

/// `ByteSource` adapter for quinn's `RecvStream`: reads discrete byte chunks
/// from a QUIC unidirectional stream. Each `poll_read` allocates a fresh
/// buffer (matching `TcpByteSource` behaviour).
pub struct QuicByteSource {
    pub recv_stream: RecvStream,
}

impl ByteSource for QuicByteSource {
    async fn poll_read(&mut self) -> Result<Option<Bytes>, String> {
        let mut buf = BytesMut::with_capacity(8 * 1024);
        match self.recv_stream.read_buf(&mut buf).await {
            Ok(0) => Ok(None),
            Ok(_n) => Ok(Some(buf.freeze())),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// 生成自签名证书
fn generate_self_signed_cert()
-> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), rcgen::Error> {
    let cert = rcgen::generate_simple_self_signed(vec!["quic_server".to_string()])?;
    let cert_der = CertificateDer::from(cert.cert);
    let pkcs8_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let key = PrivateKeyDer::Pkcs8(pkcs8_key);
    Ok((cert_der, key))
}
