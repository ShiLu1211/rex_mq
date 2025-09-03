use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use anyhow::Result;
use quinn::{
    ClientConfig, Connection, Endpoint, crypto::rustls::QuicClientConfig,
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
    client::RexClient,
    client_handler::RexClientHandler,
    command::RexCommand,
    common::{new_uuid, now_secs},
    data::RexData,
    quic_sender::QuicSender,
};

pub struct QuicClient {
    ep: Endpoint,

    // Connection和Client需要在重连时替换
    conn: RwLock<Option<Connection>>,
    client: RwLock<Option<Arc<RexClient>>>,

    // 连接配置（重连时复用）
    server_addr: SocketAddr,
    title: RwLock<String>,
    client_config: ClientConfig,
    client_handler: Arc<dyn RexClientHandler>,

    status: AtomicBool,

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
                existing_client.set_sender(Arc::new(sender)).await;
            } else {
                let id = new_uuid();
                let local_addr = self.ep.local_addr()?;
                let new_client = Arc::new(RexClient::new(
                    id,
                    local_addr,
                    self.title.read().await.clone(),
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
    async fn receiving_task(&self) {
        info!("Starting receiver task");

        loop {
            let conn = {
                let conn_guard = self.conn.read().await;
                conn_guard.clone()
            };

            if let Some(conn) = conn {
                match conn.accept_uni().await {
                    Ok(mut rcv) => {
                        debug!("Accepted incoming stream from server");

                        // 处理单个流的所有消息
                        loop {
                            let data = match RexData::read_from_quinn_stream(&mut rcv).await {
                                Ok(data) => data,
                                Err(e) => {
                                    warn!("Error reading from stream: {}", e);
                                    break;
                                }
                            };

                            if let Some(client) = self.get_client().await {
                                self.handle_received_data(&client, &data).await;
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Error accepting stream: {}", e);
                        self.status.store(false, Ordering::SeqCst);
                        break;
                    }
                }
            }

            if !self.status.load(Ordering::SeqCst) {
                info!("Attempting to reconnect...");
                if let Err(e) = self.connect().await {
                    warn!("Connection error: {}", e);
                    sleep(Duration::from_secs(1)).await;
                    continue;
                } else {
                    sleep(Duration::from_millis(100)).await;
                }
            }
        }

        info!("Receiver task ended");
    }

    async fn heartbeat_task(&self, interval: u64) {
        loop {
            sleep(Duration::from_secs(interval)).await;
            let Some(client) = self.get_client().await else {
                warn!("No client available for heartbeat");
                continue;
            };
            // 先读取 last_recv，决定是否需要发心跳
            let last = client.last_recv();
            let idle = now_secs().saturating_sub(last);
            if idle < self.idle_timeout {
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
                            let _ = self.connect().await;
                            continue;
                        }
                        let _ = s.finish();
                        debug!("Heartbeat sent, waiting for pong...");

                        // 等待 pong_wait，看 last_recv 是否被更新（收到任何数据都表示活跃）
                        let before = last;
                        sleep(Duration::from_secs(self.pong_wait)).await;
                        let after = client.last_recv();
                        if after <= before {
                            warn!("No response after heartbeat, trigger reconnect");
                            let _ = self.connect().await;
                        } else {
                            debug!("Pong (or other data) received, connection healthy");
                        }
                    }
                    Err(e) => {
                        warn!("Heartbeat open_uni failed: {}", e);
                        let _ = self.connect().await;
                        continue;
                    }
                }
            }
        }
    }

    async fn handle_received_data(&self, client: &Arc<RexClient>, data: &RexData) {
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

    async fn get_client(&self) -> Option<Arc<RexClient>> {
        let client_guard = self.client.read().await;
        client_guard.clone()
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<RexClient>,
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
