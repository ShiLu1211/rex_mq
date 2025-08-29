use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
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
use tracing::{debug, error, info, warn};

pub struct MyQuicClient {
    ep: Endpoint,
    conn: Connection,
}

impl MyQuicClient {
    pub async fn create(server_addr: SocketAddr) -> Result<Arc<Self>> {
        // 创建自定义TLS配置（跳过证书验证）
        let crypto = rustls::ClientConfig::builder()
            .dangerous()
            .with_custom_certificate_verifier(SkipServerVerification::new())
            .with_no_client_auth();

        let client_config = ClientConfig::new(Arc::new(QuicClientConfig::try_from(crypto)?));

        // 创建客户端端点
        let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 0);
        let endpoint = Endpoint::client(addr)?;

        // 连接到服务器
        let conn = endpoint
            .connect_with(client_config, server_addr, "quic_server")?
            .await?;

        info!("Client: Connected to server at {}", server_addr);

        let client = Arc::new(MyQuicClient {
            ep: endpoint,
            conn,
        });

        // 🔥 关键：启动后台接收任务（客户端持续监听服务器消息）
        let client_clone = client.clone();
        tokio::spawn(async move {
            client_clone.start_receiving().await;
            info!("Client: Receiver task stopped");
        });

        Ok(client)
    }

    // 🔥 核心方法：持续接收服务器消息
    async fn start_receiving(self: Arc<Self>) {
        info!("Client: Starting receiver task");
        loop {
            match self.conn.accept_uni().await {
                Ok(mut rcv) => {
                    debug!("Client: Accepted incoming stream from server");

                    match rcv.read_to_end(1024).await {
                        Ok(buf) => {
                            let msg = String::from_utf8_lossy(&buf);
                            // ✅ 客户端在这里接收到服务器反馈
                            info!("SERVER: {}", msg);
                        }
                        Err(e) => error!("Client: Error reading from stream: {}", e),
                    }
                }
                Err(e) => {
                    warn!("Client: Error accepting stream: {}", e);
                    break;
                }
            }
        }
        info!("Client: Receiver task ended (connection closed)");
    }

    pub async fn send(&self, msg: &str) -> Result<()> {
        info!("Client: Sending message: {}", msg);
        let mut snd = self.conn.open_uni().await?;
        snd.write_all(msg.as_bytes()).await?;
        snd.finish()?; // 正确关闭流
        debug!("Client: Message sent successfully");
        Ok(())
    }

    pub async fn close(&self) {
        info!("Client: Closing connection");
        self.conn.close(0u32.into(), b"client closing");
        self.ep.close(0u32.into(), b"client shutdown");
        self.ep.wait_idle().await;
        info!("Client: Shutdown complete");
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
