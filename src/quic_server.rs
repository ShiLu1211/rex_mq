use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use quinn::{Connection, Endpoint, ServerConfig};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use tokio::sync::{Mutex, RwLock};
use tracing::{debug, error, info, warn};

use crate::{client::RexClient, command::RexCommand, data::RexData, quic_sender::QuicSender};

pub struct QuicServer {
    ep: Endpoint,
    conns: Mutex<Vec<Connection>>,
    clients: RwLock<Vec<Arc<RexClient>>>,
}

impl QuicServer {
    pub async fn open(addr: SocketAddr) -> Result<Arc<Self>> {
        let (cert, key) = generate_self_signed_cert()?;
        let server_config = ServerConfig::with_single_cert(vec![cert], key)?;
        let endpoint = Endpoint::server(server_config, addr)?;

        let server = Arc::new(QuicServer {
            ep: endpoint.clone(),
            conns: Mutex::new(vec![]),
            clients: RwLock::new(vec![]),
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
                                let server_clone = server_.clone();
                                async move {
                                    server_clone.handle_connection(conn_).await;
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

    pub async fn close(&self) {
        self.close_clients().await;
        info!("Closing all connections");
        for conn in self.conns.lock().await.iter() {
            conn.close(0u32.into(), b"server closing");
        }
        self.ep.close(0u32.into(), b"server shutdown");
        self.ep.wait_idle().await;
        info!("Shutdown complete");
    }
}

impl QuicServer {
    async fn handle_connection(&self, conn: Connection) {
        info!("Handling new connection");
        loop {
            match conn.accept_uni().await {
                Ok(mut rcv) => {
                    debug!("Accepted incoming stream");
                    // 使用长度前缀帧协议，在一个流上可以读多条消息
                    loop {
                        let mut data = match RexData::read_from_quinn_stream(&mut rcv).await {
                            Ok(data) => data,
                            Err(e) => {
                                warn!("Error reading from stream: {}", e);
                                break;
                            }
                        };

                        match data.header().command() {
                            RexCommand::Title => {
                                let title = data.title().unwrap_or_default().to_string();
                                info!("Received title: {}", title);

                                let mut has_target = false;

                                for client in self.clients.read().await.iter() {
                                    if client.has_title(&title) {
                                        data.set_target(client.id());

                                        if let Err(e) = client.send_buf(&data.serialize()).await {
                                            warn!("Error sending to client: {}", e);
                                        } else {
                                            has_target = true;
                                            break;
                                        }
                                    }
                                }

                                if !has_target {
                                    warn!("No client found for title: {}", title);
                                }
                            }
                            RexCommand::TitleReturn => todo!(),
                            RexCommand::Group => todo!(),
                            RexCommand::GroupReturn => todo!(),
                            RexCommand::Cast => todo!(),
                            RexCommand::CastReturn => todo!(),
                            RexCommand::Login => {
                                let snd = match conn.open_uni().await {
                                    Ok(snd) => snd,
                                    Err(e) => {
                                        warn!("Error opening uni stream: {}", e);
                                        break;
                                    }
                                };
                                let client = RexClient::new(
                                    data.header().source(),
                                    conn.remote_address(),
                                    String::from_utf8_lossy(data.data()).to_string(),
                                    Arc::new(QuicSender::new(snd)),
                                );
                                let client = Arc::new(client);
                                self.add_client(client.clone()).await;

                                if let Err(e) = client
                                    .send_buf(
                                        &data.set_command(RexCommand::LoginReturn).serialize(),
                                    )
                                    .await
                                {
                                    warn!("Error sending login return: {}", e);
                                };
                            }
                            RexCommand::LoginReturn => todo!(),
                            RexCommand::Check => todo!(),
                            RexCommand::CheckReturn => todo!(),
                            RexCommand::RegTitle => todo!(),
                            RexCommand::RegTitleReturn => todo!(),
                            RexCommand::DelTitle => todo!(),
                            RexCommand::DelTitleReturn => todo!(),
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

    async fn add_client(&self, client: Arc<RexClient>) {
        let mut clients = self.clients.write().await;
        clients.push(client);
    }

    async fn close_clients(&self) {
        let clients = self.clients.write().await;
        for client in clients.iter() {
            if let Err(e) = client.close().await {
                warn!("Error closing client {}: {}", client.id(), e);
            }
        }
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
