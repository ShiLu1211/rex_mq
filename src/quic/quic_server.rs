use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use bytes::{Buf, BytesMut};
use quinn::{Connection, Endpoint, RecvStream, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::{Mutex, RwLock, oneshot};
use tracing::{debug, info, warn};

use crate::{
    client::RexClient,
    command::RexCommand,
    common::now_secs,
    data::{RetCode, RexData},
    quic::QuicSender,
};

pub struct QuicServer {
    ep: Endpoint,
    conns: Mutex<Vec<Connection>>,
    clients: RwLock<Vec<Arc<RexClient>>>,
    shutdown_tx: Mutex<Option<oneshot::Sender<()>>>,
}

impl QuicServer {
    pub async fn open(addr: SocketAddr) -> Result<Arc<Self>> {
        let (cert, key) = generate_self_signed_cert()?;
        let server_config = ServerConfig::with_single_cert(vec![cert], key)?;
        let endpoint = Endpoint::server(server_config, addr)?;

        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let server = Arc::new(QuicServer {
            ep: endpoint.clone(),
            conns: Mutex::new(vec![]),
            clients: RwLock::new(vec![]),
            shutdown_tx: Mutex::new(Some(shutdown_tx)),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server_ = server.clone();
            async move {
                info!("Accepting connections on {}", addr);
                while let Some(incoming) = endpoint.accept().await {
                    match incoming.await {
                        Ok(conn) => {
                            info!("New connection from {}", conn.remote_address());
                            server_.conns.lock().await.push(conn.clone());

                            // 为每个连接启动处理任务
                            tokio::spawn({
                                let conn_ = conn.clone();
                                let server_clone = server_.clone();
                                async move {
                                    server_clone.handle_connection(conn_).await;
                                    info!("Connection closed");
                                }
                            });
                        }
                        Err(e) => warn!("Error accepting connection: {}", e),
                    }
                }
                info!("Stopped accepting connections");
            }
        });

        tokio::spawn({
            let server_clone = server.clone();
            async move {
                let check_interval = Duration::from_secs(15); // 检查频率
                let client_timeout = 45; // 客户端超时时间（秒）
                let mut shutdown_rx = shutdown_rx;

                loop {
                    tokio::select! {
                        _ = tokio::time::sleep(check_interval) => {
                            let mut clients = server_clone.clients.write().await;
                            let now = now_secs();

                            clients.retain(|client| {
                                let last_active = client.last_recv();
                                if now - last_active > client_timeout {
                                    warn!("Client {} timed out, removing...", client.id());
                                    false
                                } else {
                                    true
                                }
                            });
                        }
                        _ = &mut shutdown_rx => {
                            info!("Cleanup task received shutdown signal, stopping.");
                            break;
                        }
                    }
                }
            }
        });

        Ok(server)
    }

    pub async fn close(&self) {
        // Send shutdown signal to the cleanup task
        if let Some(tx) = self.shutdown_tx.lock().await.take() {
            let _ = tx.send(());
        }

        self.close_clients().await;
        info!("Closing all connections");
        for conn in self.conns.lock().await.iter() {
            conn.close(0u32.into(), b"server closing");
        }
        self.ep.close(0u32.into(), b"server shutdown");
        self.ep.wait_idle().await;
        info!("Shutdown complete");
    }
}

impl QuicServer {
    async fn handle_connection(self: Arc<Self>, conn: Connection) {
        let addr = conn.remote_address();
        info!("Handling new connection: {}", addr);

        loop {
            match conn.accept_uni().await {
                Ok(rcv) => {
                    debug!("Accepted incoming stream from {}", addr);
                    let server_clone = self.clone();
                    let conn_clone = conn.clone();

                    tokio::spawn(async move {
                        if let Err(e) = server_clone.handle_stream(rcv, &conn_clone).await {
                            warn!("Error handling stream from {}: {}", addr, e);
                        }
                        debug!("Stream from {} finished", addr);
                    });
                }
                Err(e) => {
                    warn!("Connection {} closed or error: {}", addr, e);
                    break;
                }
            }
        }

        // 从连接列表中移除
        let mut conns = self.conns.lock().await;
        conns.retain(|c| c.stable_id() != conn.stable_id());
        info!("Connection {} removed from list", addr);
    }

    async fn handle_stream(&self, mut stream: RecvStream, conn: &Connection) -> Result<()> {
        // 方法1: 如果你知道最大消息大小，可以一次性读取
        // 这种方式适合 QUIC 的消息边界特性
        // match self.handle_stream_read_to_end(&mut stream, conn).await {
        //     Ok(_) => return Ok(()),
        //     Err(e) => {
        //         debug!("Read-to-end failed, trying streaming approach: {}", e);
        //         // 如果失败，尝试流式读取
        //     }
        // }

        // 方法2: 流式读取（适合大消息或未知大小的消息）
        self.handle_stream_buffered(&mut stream, conn).await
    }

    #[allow(dead_code)]
    async fn handle_stream_read_to_end(
        &self,
        stream: &mut RecvStream,
        conn: &Connection,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // QUIC 流有明确的结束标识，适合一次性读取
        const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB 限制

        let data = stream.read_to_end(MAX_MESSAGE_SIZE).await?;
        if data.is_empty() {
            return Ok(()); // 空流
        }

        let buf = BytesMut::from(&data[..]);
        let (mut rex_data, consumed) = RexData::deserialize(buf)?;

        debug!(
            "Received complete message: command={:?}, size={} bytes",
            rex_data.header().command(),
            consumed
        );

        self.handle_data(&mut rex_data, conn).await;
        Ok(())
    }

    async fn handle_stream_buffered(
        &self,
        stream: &mut RecvStream,
        conn: &Connection,
    ) -> Result<()> {
        let mut buffer = BytesMut::new();
        let mut temp_buf = vec![0u8; 4096];

        loop {
            match stream.read(&mut temp_buf).await {
                Ok(Some(n)) => {
                    buffer.extend_from_slice(&temp_buf[..n]);

                    // 尝试解析完整的数据包
                    while let Some(parse_result) = RexData::try_deserialize(&buffer) {
                        match parse_result {
                            Ok((mut data, consumed_bytes)) => {
                                debug!(
                                    "Parsed message: command={:?}, consumed {} bytes",
                                    data.header().command(),
                                    consumed_bytes
                                );

                                buffer.advance(consumed_bytes);
                                self.handle_data(&mut data, conn).await;
                            }
                            Err(e) => {
                                warn!("Error parsing data from stream: {}", e);
                                buffer.clear();
                                return Err(e.into());
                            }
                        }
                    }
                }
                Ok(None) => {
                    // 流结束
                    debug!("Stream ended");
                    break;
                }
                Err(e) => {
                    warn!("Error reading from stream: {}", e);
                    return Err(e.into());
                }
            }
        }

        // 处理缓冲区中剩余的不完整数据
        if !buffer.is_empty() {
            warn!(
                "Stream ended with {} bytes of incomplete data",
                buffer.len()
            );
        }

        Ok(())
    }

    async fn handle_data(&self, data: &mut RexData, conn: &Connection) {
        // 查找对应的客户端并更新时间戳
        let client_id = data.header().source();
        let source_client = self.find_client_by_id(client_id).await;

        if let Some(client) = &source_client {
            client.update_last_recv();
        }

        match data.header().command() {
            RexCommand::Title => {
                let title = data.title().unwrap_or_default().to_string();
                debug!("Received title: {}", title);

                let mut has_target = false;

                for client in self.clients.read().await.iter() {
                    if client.has_title(&title) {
                        // 不发送给自己
                        if client.id() == client_id {
                            continue;
                        }
                        data.set_target(client.id());

                        if let Err(e) = client.send_buf(&data.serialize()).await {
                            warn!("Error sending to client: {}", e);
                        } else {
                            has_target = true;
                            break;
                        }
                    }
                }

                if !has_target {
                    warn!("No client found for title: {}", title);
                    if let Some(client) = &source_client
                        && let Err(e) = client
                            .send_buf(
                                &data
                                    .set_command(RexCommand::TitleReturn)
                                    .set_retcode(RetCode::NoTargetAvailable)
                                    .serialize(),
                            )
                            .await
                    {
                        warn!("Error sending to client: {}", e);
                    };
                }
            }
            RexCommand::Group => {
                let title = data.title().unwrap_or_default().to_string();
                debug!("Received group: {}", title);

                let mut has_target = false;

                let matching_clients: Vec<Arc<RexClient>> = self
                    .clients
                    .read()
                    .await
                    .iter()
                    .filter(|client| client.has_title(&title) && client.id() != client_id)
                    .cloned()
                    .collect();

                // 使用轮询方式选择客户端
                static GROUP_ROUND_ROBIN_INDEX: AtomicUsize = AtomicUsize::new(0);

                let index = GROUP_ROUND_ROBIN_INDEX.fetch_add(1, Ordering::Relaxed);

                for i in index..index + matching_clients.len() {
                    let client = &matching_clients[i % matching_clients.len()];

                    data.set_target(client.id());
                    if let Err(e) = client.send_buf(&data.serialize()).await {
                        warn!("Error sending group message to client: {}", e);
                    } else {
                        debug!("Sent group message to client ID: {}", client.id());
                        has_target = true;
                        break;
                    }
                }

                if !has_target {
                    warn!("No client found for title: {}", title);
                    if let Some(client) = &source_client
                        && let Err(e) = client
                            .send_buf(
                                &data
                                    .set_command(RexCommand::GroupReturn)
                                    .set_retcode(RetCode::NoTargetAvailable)
                                    .serialize(),
                            )
                            .await
                    {
                        warn!("Error sending to client: {}", e);
                    };
                }
            }
            RexCommand::Cast => {
                let title = data.title().unwrap_or_default().to_string();
                debug!("Received cast: {}", title);

                let mut has_target = false;

                for client in self.clients.read().await.iter() {
                    if client.has_title(&title) {
                        // 不发送给自己
                        if client.id() == client_id {
                            continue;
                        }
                        data.set_target(client.id());

                        if let Err(e) = client.send_buf(&data.serialize()).await {
                            warn!("Error sending to client: {}", e);
                        } else {
                            has_target = true;
                        }
                    }
                }

                if !has_target {
                    warn!("No client found for title: {}", title);
                    if let Some(client) = &source_client
                        && let Err(e) = client
                            .send_buf(
                                &data
                                    .set_command(RexCommand::CastReturn)
                                    .set_retcode(RetCode::NoTargetAvailable)
                                    .serialize(),
                            )
                            .await
                    {
                        warn!("Error sending to client: {}", e);
                    };
                }
            }
            RexCommand::Login => {
                let snd = match conn.open_uni().await {
                    Ok(snd) => snd,
                    Err(e) => {
                        warn!("Error opening uni stream: {}", e);
                        return;
                    }
                };
                let sender = Arc::new(QuicSender::new(snd));
                if let Some(client) = &source_client {
                    client.set_sender(sender.clone());
                    if let Err(e) = client
                        .send_buf(&data.set_command(RexCommand::LoginReturn).serialize())
                        .await
                    {
                        warn!("Error sending login return: {}", e);
                    };
                } else {
                    let client = Arc::new(RexClient::new(
                        data.header().source(),
                        conn.remote_address(),
                        &data.data_as_string_lossy(),
                        sender,
                    ));
                    self.add_client(client.clone()).await;
                    if let Err(e) = client
                        .send_buf(&data.set_command(RexCommand::LoginReturn).serialize())
                        .await
                    {
                        warn!("Error sending login return: {}", e);
                    };
                }
            }
            RexCommand::Check => {
                debug!("Received check");
                if let Err(e) = self
                    .send_buf_once(conn, &data.set_command(RexCommand::CheckReturn).serialize())
                    .await
                {
                    warn!("Error sending check return: {}", e);
                } else {
                    debug!("Sent check return");
                }
            }
            RexCommand::RegTitle => {
                debug!("Received reg title");
                let title = data.data_as_string_lossy();
                if let Some(client) = &source_client {
                    client.insert_title(title);
                    if let Err(e) = client
                        .send_buf(&data.set_command(RexCommand::RegTitleReturn).serialize())
                        .await
                    {
                        warn!("Error sending reg title return: {}", e);
                    };
                } else {
                    warn!("No client found for registration");
                }
            }
            RexCommand::DelTitle => {
                debug!("Received del title");
                let title = data.data_as_string_lossy();
                if let Some(client) = &source_client {
                    client.remove_title(&title);
                    if let Err(e) = client
                        .send_buf(&data.set_command(RexCommand::DelTitleReturn).serialize())
                        .await
                    {
                        warn!("Error sending reg title return: {}", e);
                    };
                } else {
                    warn!("No client found for registration");
                }
            }
            _ => {
                debug!("Received command: {:?}", data.header().command());
            }
        }
    }

    /// 新开一个连接，发送后就关闭
    async fn send_buf_once(&self, conn: &Connection, buf: &[u8]) -> Result<()> {
        let mut snd = conn.open_uni().await?;
        snd.write_all(buf).await?;
        snd.finish()?;
        Ok(())
    }

    async fn add_client(&self, client: Arc<RexClient>) {
        let mut clients = self.clients.write().await;
        clients.push(client);
    }

    async fn close_clients(&self) {
        let clients = self.clients.write().await;
        for client in clients.iter() {
            if let Err(e) = client.close().await {
                warn!("Error closing client {}: {}", client.id(), e);
            }
        }
    }

    async fn find_client_by_id(&self, id: usize) -> Option<Arc<RexClient>> {
        let clients = self.clients.read().await;
        clients.iter().find(|client| client.id() == id).cloned()
    }
}

fn generate_self_signed_cert()
-> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), rcgen::Error> {
    let cert = rcgen::generate_simple_self_signed(vec!["quic_server".to_string()])?;
    let cert_der = CertificateDer::from(cert.cert);
    let pkcs8_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let key = PrivateKeyDer::Pkcs8(pkcs8_key);
    Ok((cert_der, key))
}
