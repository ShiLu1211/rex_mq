use std::{
    cmp::min,
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use anyhow::Result;
use rex_client::{
    ConnectionState, RexClientConfig, RexClientHandlerTrait, RexClientInner, RexClientTrait,
    open_client,
};
use rex_core::{Protocol, RexCommand, RexData};
use rex_server::{RexServerConfig, RexServerTrait, RexSystem, RexSystemConfig, open_server};
use tokio::sync::mpsc::{Receiver, Sender, channel};
use tracing::{info, warn};

/// ------------------------- Client -------------------------
pub struct TestClient {
    client: Arc<dyn RexClientTrait>,
    rx: Receiver<RexData>,
}

impl TestClient {
    pub fn new(client: Arc<dyn RexClientTrait>, rx: Receiver<RexData>) -> Self {
        Self { client, rx }
    }

    pub async fn recv(&mut self) -> Option<RexData> {
        self.rx.recv().await
    }

    pub async fn send(&self, cmd: RexCommand, title: &str, data: &[u8]) -> Result<()> {
        let mut d = RexData::builder(cmd)
            .title(title.to_string())
            .data(data.into())
            .build();
        self.client.send_data(&mut d).await
    }

    pub async fn wait_connected(&self) {
        while self.client.get_connection_state() != ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    pub async fn close(&self) {
        self.client.close().await;
    }
}

/// ------------------------- Handler -------------------------
struct TestClientHandler {
    tx: Sender<RexData>,
}

#[async_trait::async_trait]
impl RexClientHandlerTrait for TestClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        info!(
            "login ok, client id: [{:032X}], title: [{}]",
            client.id(),
            client.title_str()
        );
        Ok(())
    }

    async fn handle(&self, _client: Arc<RexClientInner>, data: RexData) -> Result<()> {
        if data.data().is_empty() {
            warn!("recv empty from [{:032X}]", data.header().source());
        } else {
            info!(
                "recv {:?}, len [{}]",
                &data.data()[..min(16, data.data().len())],
                data.data().len()
            );
        }
        if let Err(e) = self.tx.send(data).await {
            warn!("rx closed: {}", e);
        }
        Ok(())
    }
}

/// ------------------------- TestEnv -------------------------
pub struct TestEnv {
    system: Arc<RexSystem>,
    servers: HashMap<Protocol, Arc<dyn RexServerTrait>>,
    base_port: u16,
}

impl Default for TestEnv {
    fn default() -> Self {
        let _ = tracing_subscriber::fmt::try_init();
        Self {
            system: RexSystem::new(RexSystemConfig::from_id("test-system")),
            servers: HashMap::new(),
            base_port: 8880,
        }
    }
}

impl TestEnv {
    fn next_addr(&self, proto: Protocol) -> SocketAddr {
        let offset = match proto {
            Protocol::Tcp => 1,
            Protocol::Quic => 2,
            Protocol::WebSocket => 3,
        };
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.base_port + offset))
    }

    /// 启动指定协议的 server，并加入系统
    pub async fn start_server(&mut self, proto: Protocol) -> Result<Arc<dyn RexServerTrait>> {
        let addr = self.next_addr(proto);
        let cfg = RexServerConfig::new(proto, addr);
        let server = open_server(self.system.clone(), cfg).await?;
        self.servers.insert(proto, server.clone());
        Ok(server)
    }

    /// 为指定协议创建 client
    pub async fn create_client(&self, proto: Protocol, title: &str) -> Result<TestClient> {
        let addr = self.next_addr(proto);
        let (tx, rx) = channel(100);
        let handler = Arc::new(TestClientHandler { tx });
        let cfg = RexClientConfig::new(proto, addr, title.into(), handler);
        let client = open_client(cfg).await?;
        Ok(TestClient::new(client, rx))
    }

    /// 启动所有协议的聚合 server（AggregateServer）
    pub async fn start_aggregate_server(&mut self, protos: &[Protocol]) -> Result<()> {
        for &p in protos {
            self.start_server(p).await?;
        }
        Ok(())
    }

    pub async fn close_server(&mut self, proto: Protocol) {
        if let Some(s) = self.servers.remove(&proto) {
            s.close().await;
            drop(s);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
    }

    pub async fn shutdown(&mut self) {
        for (_, s) in self.servers.drain() {
            s.close().await;
        }
        self.system.close().await;
    }
}
