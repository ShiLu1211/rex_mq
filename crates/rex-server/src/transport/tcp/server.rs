use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{
    RexClientInner, RexData, RexFrame, RexFramer, RexSender, WriteCommand, utils::new_uuid,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
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

    worker_tx: kanal::AsyncSender<RexFrame>,
}

#[async_trait::async_trait]
impl RexServerTrait for TcpServer {
    async fn close(&self) {
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

        let (worker_tx, worker_rx) = kanal::bounded_async(16384);
        let (shutdown_tx, mut shutdown_rx) = broadcast::channel(1 + config.max_concurrent_handlers);

        let server = Arc::new(TcpServer {
            system,
            config,
            listener: Arc::new(listener),
            semaphore,
            shutdown_tx: Arc::new(shutdown_tx),
            worker_tx,
        });

        // 单个 worker 线程
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

    async fn handle_connection(self: &Arc<Self>, stream: TcpStream, peer_addr: SocketAddr) {
        info!("New connection from {}", peer_addr);

        if let Err(e) = stream.set_nodelay(true) {
            warn!("Error setting TCP_NODELAY for {}: {}", peer_addr, e);
        }

        let (reader, mut writer) = stream.into_split();

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
                        let _ = writer.shutdown().await;
                        debug!("Close command received for {}", peer_addr);
                        break;
                    }
                }
            }
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
                    .handle_connection_inner(peer.clone(), reader)
                    .await;

                let client_id = peer.id();
                server_clone.system.remove_client(client_id).await;

                info!("Connection {} closed and cleaned up", peer_addr);
            }
        });
    }

    async fn handle_connection_inner(&self, peer: Arc<RexClientInner>, mut reader: OwnedReadHalf) {
        let peer_addr = peer.local_addr();
        info!("Handling new connection: {}", peer_addr);

        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut framer = RexFramer::new(self.config.max_buffer_size);
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        loop {
            tokio::select! {
                // 从 TCP 流中读取数据
                result = reader.read_buf(&mut buffer) => {
                    match result {
                        Ok(0) => {
                            info!("Connection {} closed by client", peer_addr);
                            break;
                        }
                        Ok(_) => {
                            loop {
                                match framer.try_next_frame(&mut buffer) {
                                Ok(Some(payload)) => {
                                    if let Err(e) = self.worker_tx
                                        .send(RexFrame { peer: peer.clone(), payload })
                                        .await {
                                            warn!("worker_tx send error: {}", e);
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
