use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use quinn::{Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

pub struct MyQuicServer {
    ep: Endpoint,
    conns: Mutex<Vec<Connection>>,
}

impl MyQuicServer {
    pub async fn open(addr: SocketAddr) -> Result<Arc<Self>> {
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

    pub async fn close(&self) {
        info!("Server: Closing all connections");
        for conn in self.conns.lock().await.iter() {
            conn.close(0u32.into(), b"server closing");
        }
        self.ep.close(0u32.into(), b"server shutdown");
        self.ep.wait_idle().await;
        info!("Server: Shutdown complete");
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
