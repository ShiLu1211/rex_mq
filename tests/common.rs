use std::{cmp::min, net::SocketAddr, sync::Arc};

use anyhow::Result;
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tracing::{info, warn};

use rex_mq::{
    QuicClient, QuicServer, RexClient, RexClientConfig, RexClientHandler, RexClientInner,
    RexServer, RexServerConfig, RexSystem, RexSystemConfig, TcpClient, TcpServer,
    protocol::{RexCommand, RexData},
};

#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Tcp,
    Quic,
}

pub struct TestClient {
    client: Arc<dyn RexClient>,
    rx: Receiver<RexData>,
}

impl TestClient {
    pub fn new(client: Arc<dyn RexClient>, rx: Receiver<RexData>) -> Self {
        TestClient { client, rx }
    }

    pub async fn recv(&mut self) -> Option<RexData> {
        self.rx.recv().await
    }

    pub async fn send_data(&self, data: &mut RexData) -> Result<()> {
        self.client.send_data(data).await
    }

    pub async fn send(&self, command: RexCommand, title: &str, data: &[u8]) -> Result<()> {
        let mut rex_data = RexData::builder(command)
            .title(title.to_string())
            .data(data.into())
            .build();
        self.send_data(&mut rex_data).await
    }

    pub async fn close(&self) {
        self.client.close().await;
    }
}

struct TestClientHandler {
    tx: Sender<RexData>,
}

#[async_trait::async_trait]
impl RexClientHandler for TestClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        info!(
            "login ok, client id: [{:032X}], title: [{}]",
            client.id(),
            client.title_str()
        );
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        let msg = data.data();
        if msg.is_empty() {
            warn!(
                "recv empty data, from [{:032X}], command: {:?}, retcode: {:?}",
                data.header().source(),
                data.header().command(),
                data.header().retcode()
            );
            return Ok(());
        }
        info!(
            "recv from [{:032X}], data: {:?}, data_len: {}",
            data.header().source(),
            &msg[..min(16, msg.len())],
            msg.len()
        );
        if let Err(e) = self.tx.send(data).await {
            warn!("channel send data error: {}", e);
        }
        Ok(())
    }
}

pub struct TestFactory {
    server_addr: SocketAddr,
    system: Arc<RexSystem>,
}

impl Default for TestFactory {
    fn default() -> Self {
        TestFactory::new(SocketAddr::from(([127, 0, 0, 1], 8881)))
    }
}

impl TestFactory {
    pub fn new(addr: SocketAddr) -> Self {
        let _ = tracing_subscriber::fmt::try_init();
        TestFactory {
            server_addr: addr,
            system: RexSystem::new(RexSystemConfig::from_id("server")),
        }
    }

    pub async fn create_server(&self, protocol: Protocol) -> Result<Arc<dyn RexServer>> {
        let config = RexServerConfig::from_addr(self.server_addr);
        match protocol {
            Protocol::Tcp => {
                let server = TcpServer::open(self.system.clone(), config).await?;
                Ok(server)
            }
            Protocol::Quic => {
                let server = QuicServer::open(self.system.clone(), config).await?;
                Ok(server)
            }
        }
    }

    pub async fn create_client(&self, title: &str, protocol: Protocol) -> Result<TestClient> {
        let (tx, rx) = channel(10);
        let handler = Arc::new(TestClientHandler { tx });
        let config = RexClientConfig::new(self.server_addr, title.into(), handler);
        match protocol {
            Protocol::Tcp => {
                let client = TcpClient::new(config)?;
                let client = client.open().await?;

                Ok(TestClient::new(client, rx))
            }
            Protocol::Quic => {
                let client = QuicClient::new(config)?;
                let client = client.open().await?;

                Ok(TestClient::new(client, rx))
            }
        }
    }

    pub async fn close(&self) {
        self.system.close().await;
    }
}
