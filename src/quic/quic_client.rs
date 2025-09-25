use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use bytes::{Buf, BytesMut};
use quinn::{
    ClientConfig, Connection, Endpoint, RecvStream, crypto::rustls::QuicClientConfig,
    rustls::crypto::CryptoProvider,
};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger,
    crypto::{verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio::{sync::RwLock, time::sleep};
use tracing::{debug, info, warn};

use crate::{
    ClientInner, QuicSender, RexClientHandler,
    protocol::{RexCommand, RexData},
    utils::{new_uuid, now_secs},
};

pub struct QuicClient {
    ep: Endpoint,

    // Connection和Client需要在重连时替换
    conn: RwLock<Option<Connection>>,
    client: RwLock<Option<Arc<ClientInner>>>,

    // 连接配置（重连时复用）
    server_addr: SocketAddr,
    title: RwLock<String>,
    client_config: ClientConfig,
    client_handler: Arc<dyn RexClientHandler>,

    // 状态
    status: AtomicBool,
    shutdown: AtomicBool,

    // 心跳
    idle_timeout: u64,
    pong_wait: u64,
}

impl QuicClient {
    pub async fn new(
        server_addr: SocketAddr,
        title: String,
        handler: Arc<dyn RexClientHandler>,
    ) -> Result<Arc<Self>> {
        let local_addr = SocketAddr::from(([0, 0, 0, 0], 0));
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();

        let client_config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto)?));
        Ok(Arc::new(Self {
            ep: Endpoint::client(local_addr)?,
            conn: RwLock::new(None),
            client: RwLock::new(None),
            server_addr,
            title: RwLock::new(title),
            client_config,
            client_handler: handler,
            status: AtomicBool::new(false),
            shutdown: AtomicBool::new(false),
            idle_timeout: 10,
            pong_wait: 5,
        }))
    }

    pub async fn open(self: Arc<Self>) -> Result<Arc<Self>> {
        // 连接到服务器
        self.connect().await?;

        // 关键：启动后台接收任务（客户端持续监听服务器消息）
        tokio::spawn({
            let self_clone = self.clone();
            async move {
                self_clone.receiving_task().await;
                info!("Receiver task stopped");
            }
        });

        tokio::spawn({
            let interval = 15;
            let self_clone = self.clone();
            async move {
                self_clone.heartbeat_task(interval).await;
                info!("Heartbeat task stopped");
            }
        });

        Ok(self)
    }

    pub async fn send_data(&self, data: &mut RexData) -> Result<()> {
        if let Some(client) = self.get_client().await {
            self.send_data_with_client(&client, data).await
        } else {
            Err(anyhow::anyhow!("No active connection"))
        }
    }

    pub async fn close(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(client) = self.get_client().await
            && let Err(e) = client.close().await
        {
            warn!("Error closing client sender: {}", e);
        }

        if let Some(conn) = self.conn.read().await.as_ref() {
            conn.close(0u32.into(), b"client closing");
        }

        self.ep.close(0u32.into(), b"client shutdown");
        self.ep.wait_idle().await;

        info!("QuicClient shutdown complete");
    }
}

impl QuicClient {
    async fn connect(&self) -> Result<()> {
        info!("Connecting to server at {}", self.server_addr);

        // 建立新连接
        let conn = self
            .ep
            .connect_with(self.client_config.clone(), self.server_addr, "quic_server")?
            .await?;

        // 创建新的发送流和客户端
        let tx = conn.open_uni().await?;
        let sender = QuicSender::new(tx);

        // 更新连接和客户端（原子操作）
        {
            let mut conn_guard = self.conn.write().await;
            let mut client_guard = self.client.write().await;

            *conn_guard = Some(conn);

            if let Some(existing_client) = client_guard.as_ref() {
                existing_client.set_sender(Arc::new(sender));
            } else {
                let id = new_uuid();
                let local_addr = self.ep.local_addr()?;
                let new_client = Arc::new(ClientInner::new(
                    id,
                    local_addr,
                    &self.title.read().await,
                    Arc::new(sender),
                ));
                *client_guard = Some(new_client);
            }
        }

        // 登录
        self.login().await?;

        Ok(())
    }

    async fn login(&self) -> Result<()> {
        if let Some(client) = self.get_client().await {
            let mut data = RexData::builder(RexCommand::Login)
                .data_from_string(self.title.read().await.clone())
                .build();
            self.send_data_with_client(&client, &mut data).await?;
        } else {
            warn!("Client not found");
        }
        Ok(())
    }

    // 🔥 核心方法：持续接收服务器消息
    async fn receiving_task(self: Arc<Self>) {
        info!("Starting receiver task");
        let mut backoff = 1;

        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                info!("Shutdown requested, stop receiving_task");
                break;
            }

            let conn = {
                let conn_guard = self.conn.read().await;
                conn_guard.clone()
            };

            if let Some(conn) = conn {
                match conn.accept_uni().await {
                    Ok(rcv) => {
                        debug!("Accepted incoming stream from server");
                        backoff = 1;

                        // 🔥 关键：一旦成功接收流，立即设置状态为连接正常
                        self.status.store(true, Ordering::SeqCst);

                        let self_clone = self.clone();
                        tokio::spawn(async move {
                            if let Err(e) = self_clone.handle_stream(rcv).await {
                                warn!("Error handling stream : {}", e);
                            }
                            debug!("Stream task ended");
                        });
                    }
                    Err(e) => {
                        warn!("Error accepting stream: {}", e);
                        self.status.store(false, Ordering::SeqCst);

                        // 🔥 accept_uni 失败才需要重连
                        info!("Attempting to reconnect in {backoff}s...");
                        sleep(Duration::from_secs(backoff)).await;

                        match self.connect().await {
                            Ok(_) => {
                                backoff = 1;
                            }
                            Err(e) => {
                                warn!("Reconnect failed: {}", e);
                                backoff = (backoff * 2).min(60);
                            }
                        }
                    }
                }
            } else {
                // 无连接时尝试重连
                info!("No connection, attempting to reconnect in {backoff}s...");
                sleep(Duration::from_secs(backoff)).await;

                match self.connect().await {
                    Ok(_) => {
                        backoff = 1;
                    }
                    Err(e) => {
                        warn!("Reconnect failed: {}", e);
                        backoff = (backoff * 2).min(60);
                    }
                }
            }
        }

        info!("Receiver task ended");
    }

    async fn handle_stream(&self, mut stream: RecvStream) -> Result<()> {
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
        self.handle_stream_buffered(&mut stream).await
    }

    #[allow(dead_code)]
    async fn handle_stream_read_to_end(
        &self,
        stream: &mut RecvStream,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        // QUIC 流有明确的结束标识，适合一次性读取
        const MAX_MESSAGE_SIZE: usize = 1024 * 1024; // 1MB 限制

        let data = stream.read_to_end(MAX_MESSAGE_SIZE).await?;
        if data.is_empty() {
            return Ok(()); // 空流
        }

        let buf = BytesMut::from(&data[..]);
        let (rex_data, consumed) = RexData::deserialize(buf)?;

        debug!(
            "Received complete message: command={:?}, size={} bytes",
            rex_data.header().command(),
            consumed
        );

        self.handle_data(&rex_data).await;
        Ok(())
    }

    async fn handle_stream_buffered(&self, stream: &mut RecvStream) -> Result<()> {
        let mut buffer = BytesMut::new();
        let mut temp_buf = vec![0u8; 4096];

        loop {
            match stream.read(&mut temp_buf).await {
                Ok(Some(n)) => {
                    buffer.extend_from_slice(&temp_buf[..n]);

                    // 尝试解析完整的数据包
                    while let Some(parse_result) = RexData::try_deserialize(&buffer) {
                        match parse_result {
                            Ok((data, consumed_bytes)) => {
                                debug!(
                                    "Parsed message: command={:?}, consumed {} bytes",
                                    data.header().command(),
                                    consumed_bytes
                                );

                                buffer.advance(consumed_bytes);
                                self.handle_data(&data).await;
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

    async fn heartbeat_task(&self, interval: u64) {
        let mut backoff = 1;
        loop {
            if self.shutdown.load(Ordering::SeqCst) {
                info!("Shutdown requested, stop heartbeat task");
                break;
            }

            sleep(Duration::from_secs(interval)).await;
            let Some(client) = self.get_client().await else {
                warn!("No client available for heartbeat");
                continue;
            };
            // 先读取 last_recv，决定是否需要发心跳
            let last = client.last_recv();
            let idle = now_secs().saturating_sub(last);
            if idle < self.idle_timeout {
                backoff = 1; // 有数据 -> 重置退避
                // 最近已经收到数据，不需要发心跳
                continue;
            }

            // 构造 Check 数据并序列化
            let ping = RexData::builder(RexCommand::Check).build().serialize();

            let conn = {
                let conn_guard = self.conn.read().await;
                conn_guard.clone()
            };

            // 发送心跳（每次打开临时单向流）
            if let Some(conn) = conn {
                match conn.open_uni().await {
                    Ok(mut s) => {
                        if let Err(e) = s.write_all(&ping).await {
                            warn!("Heartbeat write failed: {}", e);
                            let _ = s.finish();
                            // 重连带退避
                            sleep(Duration::from_secs(backoff)).await;
                            if let Err(e) = self.connect().await {
                                warn!("Reconnect after heartbeat failed: {}", e);
                                backoff = (backoff * 2).min(60);
                            } else {
                                backoff = 1;
                            }
                            continue;
                        }
                        let _ = s.finish();
                        debug!("Heartbeat sent, waiting for pong...");

                        // 等待 pong_wait，看 last_recv 是否被更新（收到任何数据都表示活跃）
                        let before = last;
                        sleep(Duration::from_secs(self.pong_wait)).await;
                        let after = client.last_recv();
                        if after <= before {
                            warn!(
                                "No response after heartbeat, trigger reconnect in {backoff}s..."
                            );
                            sleep(Duration::from_secs(backoff)).await;
                            if let Err(e) = self.connect().await {
                                warn!("Reconnect after heartbeat failed: {}", e);
                                backoff = (backoff * 2).min(60);
                            } else {
                                backoff = 1;
                            }
                        } else {
                            debug!("Pong (or other data) received, connection healthy");
                            backoff = 1;
                        }
                    }
                    Err(e) => {
                        warn!("Heartbeat open_uni failed: {}", e);
                        sleep(Duration::from_secs(backoff)).await;
                        if let Err(e) = self.connect().await {
                            warn!("Reconnect after heartbeat failed: {}", e);
                            backoff = (backoff * 2).min(60);
                        } else {
                            backoff = 1;
                        }
                        continue;
                    }
                }
            }
        }
    }

    async fn handle_data(&self, data: &RexData) {
        let Some(client) = self.get_client().await else {
            warn!("No client available for handling data");
            return;
        };
        let handler = self.client_handler.clone();
        match data.header().command() {
            RexCommand::LoginReturn => {
                self.status.store(true, Ordering::SeqCst);
                info!("Login successful");
                if let Err(e) = handler.login_ok(client.clone(), data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data.data_as_string_lossy();
                client.insert_title(title);
                *self.title.write().await = client.title_str();
            }
            RexCommand::DelTitleReturn => {
                let title = data.data_as_string_lossy();
                client.remove_title(&title);
                *self.title.write().await = client.title_str();
            }
            RexCommand::Title
            | RexCommand::TitleReturn
            | RexCommand::Group
            | RexCommand::GroupReturn
            | RexCommand::Cast
            | RexCommand::CastReturn => {
                debug!("Received: {:?}", data.data());
                if let Err(e) = handler.handle(client.clone(), data).await {
                    warn!("Error in handle: {}", e);
                }
            }
            RexCommand::CheckReturn => {
                debug!("Received heartbeat response");
                // 心跳响应，连接正常
            }
            _ => {}
        }
        client.update_last_recv();
    }

    async fn get_client(&self) -> Option<Arc<ClientInner>> {
        let client_guard = self.client.read().await;
        client_guard.clone()
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<ClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        data.set_source(client.id());
        client.send_buf(&data.serialize()).await?;
        debug!("Data sent successfully");
        Ok(())
    }
}

#[derive(Debug)]
struct SkipServerVerification(Arc<CryptoProvider>);

impl SkipServerVerification {
    fn new() -> Arc<Self> {
        Arc::new(Self(Arc::new(rustls::crypto::ring::default_provider())))
    }
}

impl danger::ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp: &[u8],
        _now: UnixTime,
    ) -> Result<danger::ServerCertVerified, rustls::Error> {
        Ok(danger::ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<danger::HandshakeSignatureValid, rustls::Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<danger::HandshakeSignatureValid, rustls::Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}
