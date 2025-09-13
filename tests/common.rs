use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use tokio::sync::mpsc::{Receiver, Sender, channel};

use rex_mq::{QuicClient, QuicServer, RexClient, RexClientHandler, RexCommand, RexData};
use tracing::info;

pub struct TestClient {
    client: Arc<QuicClient>,
    rx: Receiver<RexData>,
}

impl TestClient {
    pub fn new(client: Arc<QuicClient>, rx: Receiver<RexData>) -> Self {
        TestClient { client, rx }
    }

    pub async fn recv(&mut self) -> Option<RexData> {
        self.rx.recv().await
    }

    pub async fn send_data(&self, data: &mut RexData) -> Result<()> {
        self.client.send_data(data).await
    }

    pub async fn send(&self, command: RexCommand, title: &str, data: &[u8]) -> Result<()> {
        let mut rex_data =
            RexData::new_with_title(command, 0, 0, title.into(), BytesMut::from(data));
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
    async fn login_ok(&self, client: Arc<RexClient>, _data: &RexData) -> Result<()> {
        info!(
            "login ok, client id: [{}], title: [{}]",
            client.id(),
            client.title_str()
        );
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClient>, data: &RexData) -> Result<()> {
        let mut msg = data.data();
        if msg.len() > 16 {
            msg = &msg[..16];
        }
        info!("recv from [{}], data: {:?}", data.header().source(), msg);
        self.tx.send(data.clone()).await?;
        Ok(())
    }
}

pub struct TestFactory {
    server_addr: SocketAddr,
}

impl Default for TestFactory {
    fn default() -> Self {
        TestFactory::new(SocketAddr::from(([127, 0, 0, 1], 8881)))
    }
}

impl TestFactory {
    pub fn new(addr: SocketAddr) -> Self {
        tracing_subscriber::fmt::init();
        TestFactory { server_addr: addr }
    }

    pub async fn create_server(&self) -> Result<Arc<QuicServer>> {
        let server = QuicServer::open(self.server_addr).await?;
        Ok(server)
    }

    pub async fn create_client(&self, title: &str) -> Result<TestClient> {
        let (tx, rx) = channel(10);
        let handler = Arc::new(TestClientHandler { tx });
        let client = QuicClient::new(self.server_addr, title.into(), handler).await?;
        let client = client.open().await?;
        Ok(TestClient::new(client, rx))
    }
}
