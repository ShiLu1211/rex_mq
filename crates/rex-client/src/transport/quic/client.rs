use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, crypto::rustls::QuicClientConfig};
use rex_core::{
    RexClientInner, RexCommand, RexData, RexDataRef, RexFrame, RexFramer, RexSender, WriteCommand,
    utils::{new_uuid, now_secs},
};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger,
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::broadcast,
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::{ConnectionState, RexClientConfig, RexClientTrait};

pub struct QuicClient {
    // connection
    endpoint: Endpoint,
    connection: arc_swap::ArcSwap<Option<Connection>>,
    client: arc_swap::ArcSwap<Option<Arc<RexClientInner>>>,
    connection_state: AtomicU8,

    // config
    config: RexClientConfig,
    client_config: ClientConfig,

    // state management
    shutdown_tx: broadcast::Sender<()>,
    last_heartbeat: AtomicU64,

    worker_tx: kanal::AsyncSender<RexFrame>,
}

#[async_trait::async_trait]
impl RexClientTrait for QuicClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = self.get_connection_state();

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.get_client() {
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
        if let Some(client) = self.get_client()
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        // 关闭QUIC连接
        if let Some(conn) = self.get_connection() {
            conn.close(0u32.into(), b"client shutdown");
        }

        // 关闭endpoint
        self.endpoint.close(0u32.into(), b"client shutdown");
        self.endpoint.wait_idle().await;

        // 更新状态
        self.set_connection_state(ConnectionState::Disconnected);

        info!("QuicClient shutdown complete");
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.connection_state.load(Ordering::Relaxed).into()
    }
}

impl QuicClient {
    pub async fn open(config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
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

        let (worker_tx, worker_rx) = kanal::bounded_async(8192);

        let client = Arc::new(Self {
            endpoint,
            connection: arc_swap::ArcSwap::from_pointee(None),
            client: arc_swap::ArcSwap::from_pointee(None),
            connection_state: AtomicU8::new(ConnectionState::Disconnected as u8),
            config,
            client_config,
            shutdown_tx,
            last_heartbeat: AtomicU64::new(now_secs()),
            worker_tx,
        });

        tokio::spawn({
            let client = client.clone();
            async move {
                client.worker_task(worker_rx).await;
            }
        });

        // 初始连接
        client.connect_with_retry().await?;

        // 启动连接监控任务
        tokio::spawn({
            let this = client.clone();
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
            let this = client.clone();
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

        Ok(client)
    }

    async fn worker_task(self: &Arc<Self>, rx: kanal::AsyncReceiver<RexFrame>) {
        while let Ok(frame) = rx.recv().await {
            let msg = RexData::as_archived(&frame.payload);
            self.handle_archieve_data(&frame.peer, msg).await;
        }
    }

    #[inline(always)]
    fn set_connection_state(&self, state: ConnectionState) {
        self.connection_state.store(state as u8, Ordering::Relaxed);
    }

    #[inline(always)]
    fn get_client(&self) -> Option<Arc<RexClientInner>> {
        self.client.load().as_ref().as_ref().cloned()
    }

    #[inline(always)]
    fn get_connection(&self) -> Option<Connection> {
        self.connection.load().as_ref().as_ref().cloned()
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
        self.set_connection_state(ConnectionState::Connecting);

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
        let mut writer = conn.open_uni().await?;

        let (tx, rx) = kanal::bounded_async(10000);

        tokio::spawn(async move {
            debug!("Writer loop started for {}", local_addr);
            while let Ok(cmd) = rx.recv().await {
                match cmd {
                    WriteCommand::Data(buf) => {
                        if let Err(e) = writer.write_all(&buf).await {
                            warn!("Write error to {}: {}, stopping writer loop", local_addr, e);
                            break;
                        }
                    }
                    WriteCommand::Close => {
                        debug!("Close command received for {}", local_addr);
                        let _ = writer.shutdown().await;
                        break;
                    }
                }
            }
            // 循环结束（Channel被Drop或出错），确保关闭 Socket
            debug!("Writer loop ended for {}", local_addr);
        });

        let sender = Arc::new(RexSender::new(tx));

        let new_client = Arc::new(RexClientInner::new(
            new_uuid(),
            local_addr,
            &self.config.title().await,
            sender.clone(),
        ));

        self.client.store(Arc::new(Some(new_client)));

        // 保存连接
        self.connection.store(Arc::new(Some(conn.clone())));

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
                this.set_connection_state(ConnectionState::Disconnected);
            }
        });

        // 执行登录
        self.login().await?;

        // 更新连接状态
        self.set_connection_state(ConnectionState::Connected);
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

    async fn handle_stream(&self, mut recv_stream: RecvStream) -> Result<()> {
        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut framer = RexFramer::new(self.config.max_buffer_size);

        let Some(client) = self.get_client() else {
            anyhow::bail!("client not found");
        };

        loop {
            match recv_stream.read_buf(&mut buffer).await {
                Ok(0) => {
                    if !buffer.is_empty() {
                        warn!("Stream ended with {} bytes remaining", buffer.len());
                    }
                    break;
                }
                Ok(_) => {
                    // 尝试解析完整的数据包
                    loop {
                        match framer.try_next_frame(&mut buffer) {
                            Ok(Some(payload)) => {
                                let frame = RexFrame {
                                    peer: client.clone(),
                                    payload,
                                };
                                if let Err(e) = self.worker_tx.send(frame).await {
                                    warn!("Failed to send frame to worker: {}", e);
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                warn!("Framing error: {}", e);
                                buffer.clear();
                                break;
                            }
                        }
                    }
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

            let current_state = self.get_connection_state();

            match current_state {
                ConnectionState::Disconnected => {
                    info!("Connection lost, attempting to reconnect...");
                    self.set_connection_state(ConnectionState::Reconnecting);

                    if let Err(e) = self.connect_with_retry().await {
                        warn!("Reconnection failed: {}", e);
                        self.set_connection_state(ConnectionState::Disconnected);
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

            if self.get_connection_state() != ConnectionState::Connected {
                continue;
            }

            let Some(client) = self.get_client() else {
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

            if let Err(e) = client.send_buf(&ping).await {
                warn!("Heartbeat send failed: {}", e);
                self.set_connection_state(ConnectionState::Disconnected);
                continue;
            }

            // 等待心跳响应
            let before_ping = last_recv;
            sleep(Duration::from_secs(self.config.pong_wait)).await;
            let after_ping = client.last_recv();

            if after_ping <= before_ping {
                warn!("Heartbeat timeout, marking connection as lost");
                self.set_connection_state(ConnectionState::Disconnected);
            } else {
                debug!("Heartbeat successful");
                self.last_heartbeat.store(now_secs(), Ordering::Relaxed);
            }
        }
    }

    async fn login(self: &Arc<Self>) -> Result<()> {
        if let Some(client) = self.get_client() {
            let mut data = RexData::builder(RexCommand::Login)
                .data_from_string(self.config.title().await.clone())
                .build();
            self.send_data_with_client(&client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    async fn handle_archieve_data(&self, client: &Arc<RexClientInner>, data_ref: RexDataRef<'_>) {
        debug!("Handling received data: command={:?}", data_ref.command());

        let handler = self.config.client_handler.clone();

        match data_ref.command() {
            RexCommand::LoginReturn => {
                info!("QUIC login successful");
                let data = data_ref.deserialize();
                if let Err(e) = handler.login_ok(client.clone(), data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data_ref.data_as_string_lossy();
                client.insert_title(&title);
                self.config.set_title(client.title_str()).await;
                info!("Title registered: {}", title);
            }
            RexCommand::DelTitleReturn => {
                let title = data_ref.data_as_string_lossy();
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
                let data = data_ref.deserialize();
                if let Err(e) = handler.handle(client.clone(), data).await {
                    warn!("Error in message handler: {}", e);
                }
            }
            _ => {
                debug!("Unhandled command: {:?}", data_ref.command());
            }
        }

        // 更新接收时间
        client.update_last_recv();
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<RexClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        let client_id = client.id();
        data.set_source(client_id);
        client.send_buf(&data.serialize()).await?;
        debug!("QUIC data sent successfully: command={:?}", data.command());
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
