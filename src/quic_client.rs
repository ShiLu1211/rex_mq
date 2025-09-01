use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    thread::sleep,
    time::Duration,
};

use anyhow::Result;
use bytes::BytesMut;
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
use tracing::{debug, info, warn};

use crate::{
    client::RexClient, client_handler::RexClientHandler, command::RexCommand, common::new_uuid,
    data::RexData, quic_sender::QuicSender,
};

pub struct QuicClient {
    ep: Endpoint,
    conn: Connection,
    client: Arc<RexClient>,
}

impl QuicClient {
    pub async fn create(
        server_addr: SocketAddr,
        title: String,
        handler: Arc<dyn RexClientHandler>,
    ) -> Result<Arc<Self>> {
        // 创建自定义TLS配置（跳过证书验证）
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();

        let client_config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto)?));

        // 创建客户端端点
        let local_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
        let endpoint = Endpoint::client(local_addr)?;

        // 连接到服务器
        let conn = endpoint
            .connect_with(client_config, server_addr, "quic_server")?
            .await?;

        let tx = conn.open_uni().await?;
        let sender = QuicSender::new(tx);

        let id = new_uuid();
        let client = RexClient::new(id, local_addr, title, Arc::new(sender));
        let client = Arc::new(client);

        info!("Connected to server at {}", server_addr);

        let quic_client = Arc::new(QuicClient {
            ep: endpoint,
            conn,
            client,
        });

        quic_client.login().await?;
        sleep(Duration::from_millis(100));

        // 🔥 关键：启动后台接收任务（客户端持续监听服务器消息）

        tokio::spawn({
            let client_clone = quic_client.clone();
            async move {
                client_clone.start_receiving(handler).await;
                info!("Receiver task stopped");
            }
        });

        Ok(quic_client)
    }

    async fn login(&self) -> Result<()> {
        let mut data = RexData::builder(RexCommand::Login)
            .data_from_string(self.client.title_str())
            .build();
        self.send_data(&mut data).await?;
        Ok(())
    }

    // 🔥 核心方法：持续接收服务器消息
    async fn start_receiving(self: Arc<Self>, handler: Arc<dyn RexClientHandler>) {
        info!("Starting receiver task");
        loop {
            match self.conn.accept_uni().await {
                Ok(mut rcv) => {
                    debug!("Accepted incoming stream from server");

                    loop {
                        let data = match RexData::read_from_quinn_stream(&mut rcv).await {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Error reading from stream: {}", e);
                                break;
                            }
                        };

                        match data.header().command() {
                            RexCommand::LoginReturn => {
                                info!("Login successful");
                                if let Err(e) = handler.login_ok(self.client.clone(), &data).await {
                                    warn!("Error in login_ok handler: {}", e);
                                }
                            }
                            RexCommand::RegTitleReturn => {
                                let title = data.data_as_string_lossy();
                                self.client.insert_title(title);
                            }

                            RexCommand::DelTitleReturn => {
                                let title = data.data_as_string_lossy();
                                self.client.remove_title(&title);
                            }

                            RexCommand::Title
                            | RexCommand::TitleReturn
                            | RexCommand::Group
                            | RexCommand::GroupReturn
                            | RexCommand::Cast
                            | RexCommand::CastReturn => {
                                debug!("Received: {:?}", data.data());
                                if let Err(e) = handler.handle(self.client.clone(), &data).await {
                                    warn!("Error in handle: {}", e);
                                }
                            }

                            RexCommand::Check => {
                                debug!("Received heartbeat check");
                                if let Err(e) = self
                                    .send(
                                        &RexData::builder(RexCommand::CheckReturn)
                                            .build()
                                            .serialize(),
                                    )
                                    .await
                                {
                                    warn!("Error sending heartbeat response: {}", e);
                                }
                            }
                            _ => {}
                        }
                    }
                }
                Err(e) => {
                    warn!("Error accepting stream: {}", e);
                    break;
                }
            }
        }
        info!("Receiver task ended (connection closed)");
    }

    pub async fn send_data(&self, data: &mut RexData) -> Result<()> {
        data.set_source(self.client.id());
        self.client.send_buf(&data.serialize()).await?;
        debug!("Data sent successfully");
        Ok(())
    }

    async fn send(&self, msg: &BytesMut) -> Result<()> {
        self.client.send_buf(msg).await?;
        debug!("Message sent successfully");
        Ok(())
    }

    pub async fn close(&self) {
        info!("Closing connection");
        if let Err(e) = self.client.close().await {
            warn!("Error closing client sender: {}", e);
        }
        self.conn.close(0u32.into(), b"client closing");
        self.ep.close(0u32.into(), b"client shutdown");
        self.ep.wait_idle().await;
        info!("Shutdown complete");
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
