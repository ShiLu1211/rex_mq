use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use quinn::{ClientConfig, Connection, Endpoint, RecvStream, crypto::rustls::QuicClientConfig};
use rex_core::{
    RexClientInner, RexData,
    utils::{force_set_value, new_uuid},
};
use rex_sender::QuicSender;
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger,
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use tokio::{io::AsyncReadExt, time::sleep};
use tracing::{debug, info, warn};

use super::base::ClientBase;
use crate::{ConnectionState, RexClientConfig, RexClientTrait};

pub struct QuicClient {
    base: ClientBase,
    endpoint: Endpoint,
    connection: Option<Connection>,
    client_config: ClientConfig,
}

#[async_trait::async_trait]
impl RexClientTrait for QuicClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = self.base.connection_state();

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.base.get_client() {
            self.base.send_data_with_client(client, data).await
        } else {
            Err(anyhow::anyhow!("No active QUIC connection"))
        }
    }

    async fn close(&self) {
        info!("Shutting down QuicClient...");

        let _ = self.base.shutdown_tx.send(());

        if let Some(client) = self.base.get_client()
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        if let Some(conn) = &self.connection {
            conn.close(0u32.into(), b"client shutdown");
        }

        self.endpoint.close(0u32.into(), b"client shutdown");
        self.endpoint.wait_idle().await;

        self.base
            .set_connection_state(ConnectionState::Disconnected);
        info!("QuicClient shutdown complete");
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.base.connection_state()
    }
}

impl QuicClient {
    pub async fn open(config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
        let (base, _) = ClientBase::new(config);

        // 配置 QUIC 客户端
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();

        let mut client_config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto)?));

        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_secs(5)));
        transport.max_idle_timeout(Some(Duration::from_secs(60).try_into()?));
        client_config.transport_config(Arc::new(transport));

        let endpoint = Endpoint::client("0.0.0.0:0".parse()?)?;

        let client = Arc::new(Self {
            base,
            endpoint,
            connection: None,
            client_config,
        });

        client.connect_with_retry().await?;

        // 启动连接监控任务
        tokio::spawn({
            let this = client.clone();
            let mut shutdown_rx = this.base.shutdown_tx.subscribe();
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
            let mut shutdown_rx = this.base.shutdown_tx.subscribe();
            async move {
                let get_state = {
                    let this = this.clone();
                    move || this.base.connection_state()
                };
                tokio::select! {
                    _ = this.base.heartbeat_task(get_state) => {
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

    async fn connect_with_retry(self: &Arc<Self>) -> Result<()> {
        let mut attempts = 0;
        let mut backoff = 1;

        loop {
            attempts += 1;

            match self.connect().await {
                Ok(_) => {
                    info!(
                        "Connected to {} after {} attempts",
                        self.base.config.server_addr, attempts
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= self.base.config.max_reconnect_attempts {
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
        self.base.set_connection_state(ConnectionState::Connecting);

        info!("Connecting QUIC to {}", self.base.config.server_addr);

        let conn = self
            .endpoint
            .connect_with(
                self.client_config.clone(),
                self.base.config.server_addr,
                "quic_server",
            )?
            .await?;

        let local_addr = self.endpoint.local_addr()?;

        let tx = conn.open_uni().await?;
        let sender = Arc::new(QuicSender::new(tx));

        // 创建或更新客户端
        {
            if let Some(existing_client) = self.base.client.as_ref() {
                existing_client.set_sender(sender.clone());
            } else {
                let id = new_uuid();
                let new_client = Arc::new(RexClientInner::new(
                    id,
                    local_addr,
                    self.base.config.title(),
                    sender.clone(),
                ));
                force_set_value(&self.base.client, Some(new_client));
            }
        }

        force_set_value(&self.connection, Some(conn.clone()));

        // 启动数据接收任务
        tokio::spawn({
            let this = Arc::clone(self);
            let mut shutdown_rx = this.base.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.receive_data_task(conn) => {
                        warn!("Data receiving task ended");
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("Data receiving task received shutdown signal");
                    }
                }

                this.base
                    .set_connection_state(ConnectionState::Disconnected);
            }
        });

        self.base.login().await?;
        self.base.set_connection_state(ConnectionState::Connected);

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
        let mut buffer = BytesMut::with_capacity(self.base.config.max_buffer_size);

        loop {
            match recv_stream.read_buf(&mut buffer).await {
                Ok(0) => {
                    debug!("Stream finished");

                    if !buffer.is_empty() {
                        warn!("Stream ended with {} bytes remaining", buffer.len());
                    }

                    break;
                }
                Ok(n) => {
                    debug!("Buffered read: {} bytes", n);
                    if let Err(e) = self.base.parse_buffer(&mut buffer).await {
                        warn!("Buffer parsing error: {}", e);
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

            let current_state = self.base.connection_state();

            match current_state {
                ConnectionState::Disconnected => {
                    info!("Connection lost, attempting to reconnect...");
                    self.base
                        .set_connection_state(ConnectionState::Reconnecting);

                    if let Err(e) = self.connect_with_retry().await {
                        warn!("Reconnection failed: {}", e);
                        self.base
                            .set_connection_state(ConnectionState::Disconnected);
                    }
                }
                ConnectionState::Reconnecting => {
                    // 重连进行中,等待
                }
                ConnectionState::Connecting | ConnectionState::Connected => {
                    // 正常状态,继续监控
                }
            }
        }
    }
}

// 跳过服务器证书验证(仅用于开发/测试)
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
