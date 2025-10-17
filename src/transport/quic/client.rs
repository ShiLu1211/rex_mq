use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use bytes::{Buf, BytesMut};
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, crypto::rustls::QuicClientConfig};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger,
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio::{
    sync::{RwLock, broadcast},
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::{
    ConnectionState, RexClient, RexClientConfig, RexClientInner,
    protocol::{RexCommand, RexData},
    utils::{new_uuid, now_secs},
};

use super::QuicSender;

pub struct QuicClient {
    // connection
    endpoint: Endpoint,
    connection: RwLock<Option<Connection>>,
    client: RwLock<Option<Arc<RexClientInner>>>,
    connection_state: Arc<RwLock<ConnectionState>>,

    // config
    config: RexClientConfig,
    client_config: ClientConfig,

    // state management
    shutdown_tx: broadcast::Sender<()>,
    last_heartbeat: AtomicU64,
}

#[async_trait::async_trait]
impl RexClient for QuicClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = *self.connection_state.read().await;

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.get_client().await {
            self.send_data_with_client(&client, data).await
        } else {
            Err(anyhow::anyhow!("No active QUIC connection"))
        }
    }

    async fn close(&self) {
        info!("Shutting down QuicClient...");

        // 发送关闭信号
        let _ = self.shutdown_tx.send(());

        // 关闭客户端连接
        if let Some(client) = self.get_client().await
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        // 关闭QUIC连接
        if let Some(conn) = self.connection.write().await.take() {
            conn.close(0u32.into(), b"client shutdown");
        }

        // 关闭endpoint
        self.endpoint.close(0u32.into(), b"client shutdown");
        self.endpoint.wait_idle().await;

        // 更新状态
        *self.connection_state.write().await = ConnectionState::Disconnected;

        info!("QuicClient shutdown complete");
    }

    async fn get_connection_state(&self) -> ConnectionState {
        *self.connection_state.read().await
    }
}

impl QuicClient {
    pub fn new(config: RexClientConfig) -> Result<Arc<Self>> {
        let (shutdown_tx, _) = broadcast::channel(4);

        // 配置 QUIC 客户端
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();

        let mut client_config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto)?));

        // 配置传输参数
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_secs(5)));
        transport.max_idle_timeout(Some(Duration::from_secs(60).try_into()?));
        client_config.transport_config(Arc::new(transport));

        // 创建 endpoint
        let endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;

        Ok(Arc::new(Self {
            endpoint,
            connection: RwLock::new(None),
            client: RwLock::new(None),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
            config,
            client_config,
            shutdown_tx,
            last_heartbeat: AtomicU64::new(now_secs()),
        }))
    }

    pub async fn open(self: Arc<Self>) -> Result<Arc<Self>> {
        // 初始连接
        self.connect_with_retry().await?;

        // 启动连接监控任务
        tokio::spawn({
            let this = self.clone();
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.connection_monitor_task() => {
                        info!("Connection monitor task completed");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Connection monitor task received shutdown signal");
                    }
                }
            }
        });

        // 启动心跳任务
        tokio::spawn({
            let this = self.clone();
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.heartbeat_task() => {
                        info!("Heartbeat task completed");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Heartbeat task received shutdown signal");
                    }
                }
            }
        });

        Ok(self)
    }

    async fn connect_with_retry(self: &Arc<Self>) -> Result<()> {
        let mut attempts = 0;
        let mut backoff = 1;

        loop {
            attempts += 1;

            match self.connect().await {
                Ok(_) => {
                    info!(
                        "Connected to {} after {} attempts",
                        self.config.server_addr, attempts
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= self.config.max_reconnect_attempts {
                        warn!("Failed to connect after {} attempts: {}", attempts, e);
                        return Err(e);
                    }

                    warn!(
                        "Connection attempt {} failed: {}, retrying in {}s",
                        attempts, e, backoff
                    );
                    sleep(Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    async fn connect(self: &Arc<Self>) -> Result<()> {
        *self.connection_state.write().await = ConnectionState::Connecting;

        info!("Connecting QUIC to {}", self.config.server_addr);

        // 建立新连接
        let conn = self
            .endpoint
            .connect_with(
                self.client_config.clone(),
                self.config.server_addr,
                "quic_server",
            )?
            .await?;

        let local_addr = self.endpoint.local_addr()?;

        // 打开第一个单向流用于发送
        let tx = conn.open_uni().await?;
        let sender = Arc::new(QuicSender::new(tx));

        // 创建或更新客户端
        {
            let mut client_guard = self.client.write().await;
            if let Some(existing_client) = client_guard.as_ref() {
                existing_client.set_sender(sender.clone());
            } else {
                let id = new_uuid();
                let new_client = Arc::new(RexClientInner::new(
                    id,
                    local_addr,
                    &self.config.title().await,
                    sender.clone(),
                ));
                *client_guard = Some(new_client);
            }
        }

        // 保存连接
        *self.connection.write().await = Some(conn.clone());

        // 启动数据接收任务
        tokio::spawn({
            let this = Arc::clone(self);
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.receive_data_task(conn) => {
                        warn!("Data receiving task ended");
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("Data receiving task received shutdown signal");
                    }
                }

                // 标记连接断开
                *this.connection_state.write().await = ConnectionState::Disconnected;
            }
        });

        // 执行登录
        self.login().await?;

        // 更新连接状态
        *self.connection_state.write().await = ConnectionState::Connected;
        self.last_heartbeat.store(now_secs(), Ordering::Relaxed);

        Ok(())
    }

    async fn receive_data_task(self: &Arc<Self>, connection: Connection) {
        info!("Starting QUIC receiver task");

        loop {
            match connection.accept_uni().await {
                Ok(recv_stream) => {
                    debug!("Accepted incoming stream from server");
                    let this = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = this.handle_stream(recv_stream).await {
                            warn!("Error handling stream: {}", e);
                        }
                    });
                }
                Err(e) => {
                    info!("QUIC connection closed: {}", e);
                    break;
                }
            }
        }

        info!("QUIC receiver task ended");
    }

    async fn handle_stream(self: &Arc<Self>, mut recv_stream: RecvStream) -> Result<()> {
        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut temp_buf = vec![0u8; self.config.read_buffer_size];
        let mut total_read = 0;

        loop {
            match recv_stream.read(&mut temp_buf).await {
                Ok(Some(n)) => {
                    total_read += n;
                    debug!("Buffered read: {} bytes (total: {})", n, total_read);

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
                                self.on_data(data).await;
                            }
                            Err(e) => {
                                warn!("Data parsing error: {}, clearing buffer", e);
                                buffer.clear();
                                break;
                            }
                        }
                    }

                    // 检查缓冲区是否过大
                    if buffer.len() > self.config.max_buffer_size {
                        warn!(
                            "Read buffer too large ({}KB), clearing",
                            buffer.len() / 1024
                        );
                        buffer.clear();
                    }
                }
                Ok(None) => {
                    debug!("Stream finished, total read: {} bytes", total_read);

                    if !buffer.is_empty() {
                        warn!("Stream ended with {} bytes remaining", buffer.len());
                    }

                    break;
                }
                Err(e) => {
                    warn!("QUIC read error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }

    async fn connection_monitor_task(self: &Arc<Self>) {
        let check_interval = Duration::from_secs(5);

        loop {
            sleep(check_interval).await;

            let current_state = *self.connection_state.read().await;

            match current_state {
                ConnectionState::Disconnected => {
                    info!("Connection lost, attempting to reconnect...");
                    *self.connection_state.write().await = ConnectionState::Reconnecting;

                    if let Err(e) = self.connect_with_retry().await {
                        warn!("Reconnection failed: {}", e);
                        *self.connection_state.write().await = ConnectionState::Disconnected;
                    }
                }
                ConnectionState::Reconnecting => {
                    // 重连进行中，等待
                }
                ConnectionState::Connecting | ConnectionState::Connected => {
                    // 正常状态，继续监控
                }
            }
        }
    }

    async fn heartbeat_task(self: &Arc<Self>) {
        let heartbeat_interval = Duration::from_secs(15);

        loop {
            sleep(heartbeat_interval).await;

            if *self.connection_state.read().await != ConnectionState::Connected {
                continue;
            }

            let Some(client) = self.get_client().await else {
                continue;
            };

            let last_recv = client.last_recv();
            let idle_time = now_secs().saturating_sub(last_recv);

            if idle_time < self.config.idle_timeout {
                continue;
            }

            // 发送心跳
            debug!("Sending heartbeat (idle: {}s)", idle_time);
            let ping = RexData::builder(RexCommand::Check).build().serialize();

            // 获取连接并打开新的单向流发送心跳
            let conn = {
                let conn_guard = self.connection.read().await;
                conn_guard.clone()
            };

            if let Some(conn) = conn {
                match conn.open_uni().await {
                    Ok(mut stream) => {
                        if let Err(e) = stream.write_all(&ping).await {
                            warn!("Heartbeat send failed: {}", e);
                            *self.connection_state.write().await = ConnectionState::Disconnected;
                            continue;
                        }
                        let _ = stream.finish();

                        // 等待心跳响应
                        let before_ping = last_recv;
                        sleep(Duration::from_secs(self.config.pong_wait)).await;
                        let after_ping = client.last_recv();

                        if after_ping <= before_ping {
                            warn!("Heartbeat timeout, marking connection as lost");
                            *self.connection_state.write().await = ConnectionState::Disconnected;
                        } else {
                            debug!("Heartbeat successful");
                            self.last_heartbeat.store(now_secs(), Ordering::Relaxed);
                        }
                    }
                    Err(e) => {
                        warn!("Failed to open stream for heartbeat: {}", e);
                        *self.connection_state.write().await = ConnectionState::Disconnected;
                    }
                }
            }
        }
    }

    async fn login(self: &Arc<Self>) -> Result<()> {
        if let Some(client) = self.get_client().await {
            let mut data = RexData::builder(RexCommand::Login)
                .data_from_string(self.config.title().await.clone())
                .build();
            self.send_data_with_client(&client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    async fn on_data(&self, data: RexData) {
        if let Some(client) = self.get_client().await {
            self.handle_received_data(&client, data).await;
        }
    }

    async fn handle_received_data(&self, client: &Arc<RexClientInner>, data: RexData) {
        debug!(
            "Handling received data: command={:?}",
            data.header().command()
        );

        let handler = self.config.client_handler.clone();

        match data.header().command() {
            RexCommand::LoginReturn => {
                info!("QUIC login successful");
                if let Err(e) = handler.login_ok(client.clone(), data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data.data_as_string_lossy();
                client.insert_title(title.clone());
                self.config.set_title(client.title_str()).await;
                info!("Title registered: {}", title);
            }
            RexCommand::DelTitleReturn => {
                let title = data.data_as_string_lossy();
                client.remove_title(&title);
                self.config.set_title(client.title_str()).await;
                info!("Title removed: {}", title);
            }
            RexCommand::Title
            | RexCommand::TitleReturn
            | RexCommand::Group
            | RexCommand::GroupReturn
            | RexCommand::Cast
            | RexCommand::CastReturn => {
                if let Err(e) = handler.handle(client.clone(), data).await {
                    warn!("Error in message handler: {}", e);
                }
            }
            _ => {
                debug!("Unhandled command: {:?}", data.header().command());
            }
        }

        // 更新接收时间
        client.update_last_recv();
    }

    async fn get_client(&self) -> Option<Arc<RexClientInner>> {
        self.client.read().await.clone()
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<RexClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        data.set_source(client.id());
        client.send_buf(&data.serialize()).await?;
        debug!(
            "QUIC data sent successfully: command={:?}",
            data.header().command()
        );
        Ok(())
    }
}

// 跳过服务器证书验证（仅用于开发/测试）
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
