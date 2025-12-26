use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use quinn::{Connection, Endpoint, RecvStream, ServerConfig};
use rex_core::{
    RexClientInner, RexData, RexFrame, RexFramer, RexSender, WriteCommand, utils::new_uuid,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::{Semaphore, broadcast},
};
use tracing::{debug, info, warn};

use crate::{RexServerConfig, RexServerTrait, RexSystem, handler::handle};

pub struct QuicServer {
    system: Arc<RexSystem>,
    config: RexServerConfig,
    endpoint: Endpoint,
    semaphore: Arc<Semaphore>,
    shutdown_tx: Arc<broadcast::Sender<()>>,

    worker_tx: kanal::AsyncSender<RexFrame>,
}

#[async_trait::async_trait]
impl RexServerTrait for QuicServer {
    async fn close(&self) {
        // Send shutdown signal to all tasks
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Error sending shutdown signal: {}", e);
        }
        // Close endpoint
        self.endpoint.close(0u32.into(), b"server shutdown");

        // Wait a bit for graceful shutdown
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.endpoint.wait_idle().await;

        info!("Shutdown complete");
    }
}

impl QuicServer {
    pub async fn open(
        system: Arc<RexSystem>,
        config: RexServerConfig,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = config.bind_addr;
        // 生成自签名证书
        let (cert, key) = generate_self_signed_cert()?;
        // 配置服务器
        let server_config = ServerConfig::with_single_cert(vec![cert], key)?;
        // 配置传输参数
        // let mut transport = quinn::TransportConfig::default();
        // transport.keep_alive_interval(Some(std::time::Duration::from_secs(5)));
        // transport.max_idle_timeout(Some(std::time::Duration::from_secs(60).try_into()?));
        // transport.max_concurrent_uni_streams(1000u32.into());
        // server_config.transport_config(Arc::new(transport));
        // 创建endpoint
        let endpoint = Endpoint::server(server_config, addr)?;
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_handlers));

        let (worker_tx, worker_rx) = kanal::bounded_async(16384);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1 + config.max_concurrent_handlers);

        let server = Arc::new(QuicServer {
            system,
            config,
            endpoint,
            semaphore,
            shutdown_tx: Arc::new(shutdown_tx),
            worker_tx,
        });

        // 单个 worker 线程 - 保持低延迟
        tokio::spawn({
            let server_ = server.clone();
            async move {
                server_.worker_task(worker_rx).await;
            }
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server_ = server.clone();
            async move {
                info!("Accepting QUIC connections on {}", addr);
                loop {
                    tokio::select! {
                        Some(incoming) = server_.endpoint.accept() => {
                            let server_clone = server_.clone();
                            tokio::spawn(async move {
                                match incoming.await {
                                    Ok(connection) => {
                                        let remote_addr = connection.remote_address();
                                        info!("New QUIC connection from {}", remote_addr);
                                        server_clone.handle_connection(connection, remote_addr).await;
                                    }
                                    Err(e) => {
                                        warn!("Failed to establish connection: {}", e);
                                    }
                                }
                            });
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

    async fn worker_task(self: &Arc<Self>, rx: kanal::AsyncReceiver<RexFrame>) {
        while let Ok(frame) = rx.recv().await {
            // 零拷贝访问 - 不进行反序列化
            let data_ref = RexData::as_archived(&frame.payload);

            // 直接传递零拷贝引用给 handler
            if let Err(e) = handle(&self.system, &frame.peer, data_ref).await {
                warn!(
                    "Error handling data from {}: {}",
                    &frame.peer.local_addr(),
                    e
                );
            }

            frame.peer.update_last_recv();
        }
    }

    async fn handle_connection(self: &Arc<Self>, connection: Connection, peer_addr: SocketAddr) {
        info!("Handling QUIC connection from {}", peer_addr);

        // 打开第一个单向流用于发送
        let mut writer = match connection.open_uni().await {
            Ok(res) => res,
            Err(e) => {
                warn!("Failed to open initial stream for {}: {}", peer_addr, e);
                return;
            }
        };

        let (tx, rx) = kanal::bounded_async(10000);

        tokio::spawn(async move {
            debug!("Writer loop started for {}", peer_addr);

            while let Ok(cmd) = rx.recv().await {
                match cmd {
                    WriteCommand::Data(buf) => {
                        if let Err(e) = writer.write_all(&buf).await {
                            warn!("Write error to {}: {}, stopping writer loop", peer_addr, e);
                            break;
                        }
                    }
                    WriteCommand::Close => {
                        debug!("Close command received for {}", peer_addr);
                        let _ = writer.shutdown().await;
                        break;
                    }
                }
            }
            // 循环结束（Channel被Drop或出错），确保关闭 Socket
            debug!("Writer loop ended for {}", peer_addr);
        });

        let sender = Arc::new(RexSender::new(tx));

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
                    .handle_connection_inner(peer.clone(), connection)
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
                    info!("Connection {} closed: {}", peer_addr, e);
                    break;
                }
            }
        }

        info!("Finished processing streams from {}", peer_addr);
    }

    async fn handle_stream(&self, peer: Arc<RexClientInner>, mut reader: RecvStream) -> Result<()> {
        let peer_addr = peer.local_addr();

        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut framer = RexFramer::new(self.config.max_buffer_size);

        loop {
            // 从 QUIC 流中读取数据
            match reader.read_buf(&mut buffer).await {
                Ok(0) => {
                    // Stream finished
                    debug!("Stream from {} finished", peer_addr);
                    break;
                }
                Ok(_) => {
                    // 尝试解析完整的数据包
                    loop {
                        match framer.try_next_frame(&mut buffer) {
                            Ok(Some(payload)) => {
                                if let Err(e) = self
                                    .worker_tx
                                    .send(RexFrame {
                                        peer: peer.clone(),
                                        payload,
                                    })
                                    .await
                                {
                                    warn!("worker_tx send error: {}", e);
                                    break;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                warn!("Framing error from {}: {}", peer.local_addr(), e);
                                buffer.clear();
                                break;
                            }
                        }
                    }
                }
                Err(e) => {
                    info!("Stream from {} read error: {}", peer_addr, e);
                    break;
                }
            }
        }

        info!("Finished processing stream from {}", peer_addr);

        Ok(())
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
