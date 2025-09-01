use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use quinn::{Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::Mutex;
use tracing::{debug, error, info, warn};

use crate::data::RexData;

pub struct QuicServer {
    ep: Endpoint,
    conns: Mutex<Vec<Connection>>,
}

impl QuicServer {
    pub async fn open(addr: SocketAddr) -> Result<Arc<Self>> {
        let (cert, key) = generate_self_signed_cert()?;
        let server_config = ServerConfig::with_single_cert(vec![cert], key)?;
        let endpoint = Endpoint::server(server_config, addr)?;

        let server = Arc::new(QuicServer {
            ep: endpoint.clone(),
            conns: Mutex::new(vec![]),
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
                                async move {
                                    QuicServer::handle_connection(conn_).await;
                                    info!("Connection closed");
                                }
                            });
                        }
                        Err(e) => error!("Error accepting connection: {}", e),
                    }
                }
                info!("Stopped accepting connections");
            }
        });

        Ok(server)
    }

    async fn handle_connection(conn: Connection) {
        info!("Handling new connection");
        loop {
            match conn.accept_uni().await {
                Ok(mut rcv) => {
                    debug!("Accepted incoming stream");
                    // 使用长度前缀帧协议，在一个流上可以读多条消息
                    loop {
                        let data = match RexData::read_from_quinn_stream(&mut rcv).await {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Error reading from stream: {}", e);
                                break;
                            }
                        };
                        let msg = String::from_utf8_lossy(data.data());
                        info!("Received from client: {}", msg);

                        // 处理消息并回显（每条回显使用新的 uni 流）
                        let response = format!("Echo: {}", msg);
                        match conn.open_uni().await {
                            Ok(mut snd) => {
                                if let Err(e) = snd.write_all(response.as_bytes()).await {
                                    error!("Error writing response: {}", e);
                                }
                                if let Err(e) = snd.finish() {
                                    error!("Error finishing response stream: {}", e);
                                }
                            }
                            Err(e) => error!("Error opening response stream: {}", e),
                        }
                    }
                }
                Err(e) => {
                    warn!("Error accepting stream: {}", e);
                    break;
                }
            }
        }
    }

    pub async fn close(&self) {
        info!("Closing all connections");
        for conn in self.conns.lock().await.iter() {
            conn.close(0u32.into(), b"server closing");
        }
        self.ep.close(0u32.into(), b"server shutdown");
        self.ep.wait_idle().await;
        info!("Shutdown complete");
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
