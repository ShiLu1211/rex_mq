use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use quinn::{
    ClientConfig, Connection, Endpoint, ServerConfig, crypto::rustls::QuicClientConfig,
    rustls::crypto::CryptoProvider,
};
use rustls::{
    DigitallySignedStruct, SignatureScheme,
    client::danger,
    crypto::{verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
};
use tokio::{sync::Mutex, time::sleep};
use tracing::{debug, error, info, warn};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    let port = 8881;
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);

    // 启动服务器
    let server = MyQuicServer::open(server_addr).await?;
    info!("Server started on {}", server_addr);

    sleep(Duration::from_secs(1)).await;

    // 创建客户端（自动启动接收任务）
    let client = MyQuicClient::create(server_addr).await?;
    info!("Client connected to server");

    // 客户端持续接收消息（后台任务已启动）

    // 模拟用户交互：发送10条消息
    for i in 0..10 {
        info!("USER: Sending message {}", i);
        client.send(&format!("Hello from client: {}", i)).await?;
        sleep(Duration::from_secs(1)).await;
    }

    info!("USER: Finished sending messages");

    // 等待一段时间让客户端接收剩余消息
    sleep(Duration::from_secs(2)).await;

    // 关闭连接
    client.close().await;
    sleep(Duration::from_secs(1)).await;
    server.close().await;

    info!("Connections closed, waiting for port release...");
    // 在 main() 结尾替换原有检查
    sleep(Duration::from_secs(3)).await;

    // 检查是否有活跃连接
    let has_active = std::process::Command::new("sh")
        .arg("-c")
        .arg(format!("lsof -i :{} | grep -v '0t0'", port))
        .output()
        .map(|o| !o.stdout.is_empty())
        .unwrap_or(true);

    if has_active {
        warn!(
            "⚠️  Port {} has active connections:\n{}",
            port,
            String::from_utf8_lossy(
                &std::process::Command::new("lsof")
                    .arg(format!("-i:{}", port))
                    .output()
                    .unwrap()
                    .stdout
            )
        );
    } else {
        info!("✅ Port {} is fully released (UDP socket closed)", port);
    }

    let _server = MyQuicServer::open(server_addr).await?;

    Ok(())
}

struct MyQuicServer {
    ep: Endpoint,
    conns: Mutex<Vec<Connection>>,
}

impl MyQuicServer {
    async fn open(addr: SocketAddr) -> Result<Arc<Self>> {
        let (cert, key) = generate_self_signed_cert()?;
        let server_config = ServerConfig::with_single_cert(vec![cert], key)?;
        let endpoint = Endpoint::server(server_config, addr)?;

        let server = Arc::new(MyQuicServer {
            ep: endpoint.clone(),
            conns: Mutex::new(vec![]),
        });

        // 服务器连接处理任务
        tokio::spawn({
            let server_ = server.clone();
            async move {
                info!("Server: Accepting connections on {}", addr);
                while let Some(incoming) = endpoint.accept().await {
                    match incoming.await {
                        Ok(conn) => {
                            info!("Server: New connection from {}", conn.remote_address());
                            server_.conns.lock().await.push(conn.clone());

                            // 为每个连接启动处理任务
                            tokio::spawn({
                                let conn_ = conn.clone();
                                async move {
                                    MyQuicServer::handle_connection(conn_).await;
                                    info!("Server: Connection closed");
                                }
                            });
                        }
                        Err(e) => error!("Server: Error accepting connection: {}", e),
                    }
                }
                info!("Server: Stopped accepting connections");
            }
        });

        Ok(server)
    }

    async fn handle_connection(conn: Connection) {
        info!("Server: Handling new connection");
        loop {
            match conn.accept_uni().await {
                Ok(mut rcv) => {
                    debug!("Server: Accepted incoming stream");

                    match rcv.read_to_end(1024).await {
                        Ok(buf) => {
                            let msg = String::from_utf8_lossy(&buf);
                            info!("Server: Received from client: {}", msg);

                            // 处理消息（这里简单回显）
                            let response = format!("Echo: {}", msg);

                            // 发送响应
                            match conn.open_uni().await {
                                Ok(mut snd) => {
                                    if let Err(e) = snd.write_all(response.as_bytes()).await {
                                        error!("Server: Error writing response: {}", e);
                                    }
                                    if let Err(e) = snd.finish() {
                                        error!("Server: Error finishing response stream: {}", e);
                                    }
                                }
                                Err(e) => error!("Server: Error opening response stream: {}", e),
                            }
                        }
                        Err(e) => error!("Server: Error reading from stream: {}", e),
                    }
                }
                Err(e) => {
                    warn!("Server: Error accepting stream: {}", e);
                    break;
                }
            }
        }
    }

    async fn close(&self) {
        info!("Server: Closing all connections");
        for conn in self.conns.lock().await.iter() {
            conn.close(0u32.into(), b"server closing");
        }
        self.ep.close(0u32.into(), b"server shutdown");
        self.ep.wait_idle().await;
        info!("Server: Shutdown complete");
    }
}

struct MyQuicClient {
    ep: Endpoint,
    conn: Connection,
}

impl MyQuicClient {
    async fn create(server_addr: SocketAddr) -> Result<Arc<Self>> {
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
            conn: conn.clone(),
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

    async fn send(&self, msg: &str) -> Result<()> {
        info!("Client: Sending message: {}", msg);
        let mut snd = self.conn.open_uni().await?;
        snd.write_all(msg.as_bytes()).await?;
        snd.finish()?; // 正确关闭流
        debug!("Client: Message sent successfully");
        Ok(())
    }

    async fn close(&self) {
        info!("Client: Closing connection");
        self.conn.close(0u32.into(), b"client closing");
        self.ep.close(0u32.into(), b"client shutdown");
        self.ep.wait_idle().await;
        info!("Client: Shutdown complete");
    }
}

// 证书生成函数保持不变
fn generate_self_signed_cert()
-> Result<(CertificateDer<'static>, PrivateKeyDer<'static>), rcgen::Error> {
    let cert = rcgen::generate_simple_self_signed(vec!["quic_server".to_string()])?;
    let cert_der = CertificateDer::from(cert.cert);
    let pkcs8_key = PrivatePkcs8KeyDer::from(cert.signing_key.serialize_der());
    let key = PrivateKeyDer::Pkcs8(pkcs8_key);
    Ok((cert_der, key))
}

// 证书验证器保持不变
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
