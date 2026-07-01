use std::{
    cmp::min,
    collections::HashMap,
    net::{Ipv4Addr, SocketAddr},
    sync::Arc,
};

use anyhow::Result;
use rex_client::{
    ConnectionState, RexClientConfig, RexClientHandlerTrait, RexClientTrait, open_client,
};
use rex_core::{Protocol, RexClientInner, RexCommand, RexData};
use rex_server::{
    ClusterConfig, RexServerConfig, RexServerTrait, RexSystem, RexSystemConfig, Shutdown,
    open_server,
};
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
        let mut d = RexData::new(cmd, title, data);
        self.client.send_data(&mut d).await
    }

    pub async fn wait_connected(&self) {
        while self.client.get_connection_state() != ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    pub fn is_connected(&self) -> bool {
        self.client.get_connection_state() == ConnectionState::Connected
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
            warn!("recv empty from [{:032X}]", data.source());
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
    shutdown: Arc<Shutdown>,
    servers: HashMap<Protocol, Arc<dyn RexServerTrait>>,
    base_port: u16,
    /// ACK enabled flag
    pub ack_enabled: bool,
    /// Cluster port counter
    pub cluster_port_counter: u16,
    /// Server/Client port counter for unique ports
    port_counter: u16,
    /// Server addresses by protocol (for clients to connect to)
    server_addrs: HashMap<Protocol, SocketAddr>,
}

impl TestEnv {
    pub async fn new() -> Self {
        let _ = tracing_subscriber::fmt::try_init();
        // Use random base port to avoid conflicts between parallel tests
        let base_port = 28800 + (rand::random::<u16>() % 1000);
        let cluster_port = 38800 + (rand::random::<u16>() % 1000);
        let shutdown = Shutdown::new();
        Self {
            system: RexSystem::new(RexSystemConfig::from_id("test-system"), shutdown.clone()).await,
            shutdown,
            servers: HashMap::new(),
            base_port,
            ack_enabled: false,
            cluster_port_counter: cluster_port,
            port_counter: 0,
            server_addrs: HashMap::new(),
        }
    }

    /// Create a new TestEnv with ACK enabled
    pub async fn new_with_ack() -> Self {
        let _ = tracing_subscriber::fmt::try_init();
        let mut config = RexSystemConfig::from_id("test-system");
        config.ack_enabled = true;
        config.ack_timeout = 5000;
        // Use random base port to avoid conflicts between parallel tests
        let base_port = 28800 + (rand::random::<u16>() % 1000);
        let cluster_port = 38800 + (rand::random::<u16>() % 1000);
        let shutdown = Shutdown::new();
        Self {
            system: RexSystem::new(config, shutdown.clone()).await,
            shutdown,
            servers: HashMap::new(),
            base_port,
            ack_enabled: true,
            cluster_port_counter: cluster_port,
            port_counter: 0,
            server_addrs: HashMap::new(),
        }
    }

    fn next_addr(&mut self, proto: Protocol) -> SocketAddr {
        let offset = self.port_counter;
        self.port_counter += 1;
        // Use base_port + offset, with protocol offset as starting point
        let proto_offset = match proto {
            Protocol::Tcp => 0,
            Protocol::Quic => 100,
            Protocol::WebSocket => 200,
        };
        SocketAddr::from((Ipv4Addr::LOCALHOST, self.base_port + proto_offset + offset))
    }

    /// 启动指定协议的 server，并加入系统
    pub async fn start_server(&mut self, proto: Protocol) -> Result<Arc<dyn RexServerTrait>> {
        let addr = self.next_addr(proto);
        let cfg = RexServerConfig::new(proto, addr);
        let server = open_server(self.system.clone(), cfg, self.shutdown.clone()).await?;
        self.servers.insert(proto, server.clone());
        self.server_addrs.insert(proto, addr);
        Ok(server)
    }

    /// 启动指定协议的 server，使用指定端口
    pub async fn start_server_with_addr(
        &mut self,
        proto: Protocol,
        addr: SocketAddr,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let cfg = RexServerConfig::new(proto, addr);
        let server = open_server(self.system.clone(), cfg, self.shutdown.clone()).await?;
        self.servers.insert(proto, server.clone());
        self.server_addrs.insert(proto, addr);
        Ok(server)
    }

    /// 启动指定协议的 cluster server
    pub async fn start_cluster_server(
        &mut self,
        proto: Protocol,
        node_id: &str,
        seed_nodes: Vec<SocketAddr>,
    ) -> Result<Arc<dyn RexServerTrait>> {
        let addr = self.next_addr(proto);
        let cluster_addr = SocketAddr::from((Ipv4Addr::LOCALHOST, self.cluster_port_counter));
        self.cluster_port_counter += 1;

        tracing::info!(
            "Starting cluster server {} at {} with cluster_addr {}",
            node_id,
            addr,
            cluster_addr
        );

        let cfg = RexServerConfig {
            protocol: proto,
            bind_addr: addr,
            enabled: true,
            max_buffer_size: 8 * 1024 * 1024,
            max_concurrent_handlers: 1000,
            cluster: Some(ClusterConfig {
                enabled: true,
                cluster_addr,
                seed_nodes,
                node_id: Some(node_id.to_string()),
            }),
        };

        let server = open_server(self.system.clone(), cfg, self.shutdown.clone()).await?;
        self.servers.insert(proto, server.clone());
        Ok(server)
    }

    /// 为指定协议创建 client
    pub async fn create_client(&mut self, proto: Protocol, title: &str) -> Result<TestClient> {
        // Use the server address for the specific protocol
        let server_addr = self
            .server_addrs
            .get(&proto)
            .copied()
            .unwrap_or_else(|| self.next_addr(proto));
        let (tx, rx) = channel(100);
        let handler = Arc::new(TestClientHandler { tx });
        let cfg = RexClientConfig::new(proto, server_addr, title, handler);
        let client = open_client(cfg).await?;
        Ok(TestClient::new(client, rx))
    }

    /// 创建连接到指定服务器地址的 client
    pub async fn create_client_to_addr(
        &mut self,
        server_addr: SocketAddr,
        title: &str,
    ) -> Result<TestClient> {
        let (tx, rx) = channel(100);
        let handler = Arc::new(TestClientHandler { tx });
        let cfg = RexClientConfig::new(Protocol::Tcp, server_addr, title, handler);
        let client = open_client(cfg).await?;
        Ok(TestClient::new(client, rx))
    }

    /// 为指定协议创建支持 ACK 的 client
    pub async fn create_client_with_ack(
        &mut self,
        proto: Protocol,
        title: &str,
    ) -> Result<TestClient> {
        // Use the server address for the specific protocol
        let server_addr = self
            .server_addrs
            .get(&proto)
            .copied()
            .unwrap_or_else(|| self.next_addr(proto));
        let (tx, rx) = channel(100);
        let handler = Arc::new(TestClientHandler { tx });
        let cfg = RexClientConfig::new(proto, server_addr, title, handler);
        let mut cfg = cfg;
        cfg.ack_enabled = self.ack_enabled;
        cfg.ack_timeout_ms = 5000;
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

    /// Close server and return its address (so it can be restarted on the same port)
    pub async fn close_server(&mut self, proto: Protocol) -> Option<SocketAddr> {
        let addr = self.server_addrs.remove(&proto);
        if let Some(s) = self.servers.remove(&proto) {
            s.close().await;
            drop(s);
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        }
        addr
    }

    pub async fn shutdown(&mut self) {
        for (_, s) in self.servers.drain() {
            s.close().await;
        }
        self.system.close().await;
    }
}
