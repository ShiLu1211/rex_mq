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
use rex_observability::ObservabilityConfig;
use rex_server::{
    ClusterConfig, RexServerConfig, RexServerTrait, RexSystemConfig, Services, Shutdown,
    build_services, open_server,
};
use tokio::sync::{
    Notify,
    mpsc::{Receiver, Sender, channel},
};
use tracing::{info, warn};

/// ------------------------- Client -------------------------
pub struct TestClient {
    client: Arc<dyn RexClientTrait>,
    rx: Receiver<RexData>,
    /// Signalled when the server's `LoginReturn` is received — i.e. the
    /// title has been registered server-side. `wait_logged_in` waits on
    /// this; tests should call it before publishing to ensure the receiver
    /// route exists.
    logged_in: Arc<Notify>,
}

impl TestClient {
    pub fn new(
        client: Arc<dyn RexClientTrait>,
        rx: Receiver<RexData>,
        logged_in: Arc<Notify>,
    ) -> Self {
        Self {
            client,
            rx,
            logged_in,
        }
    }

    /// Unbounded receive — a missed message will hang the caller forever.
    /// Prefer `recv_timeout(d)` from test code.
    pub async fn recv(&mut self) -> Option<RexData> {
        self.rx.recv().await
    }

    /// Bounded receive. Returns `Ok(None)` on timeout, `Ok(Some(data))` on
    /// success, `Err(reason)` on channel closure. Use this instead of bare
    /// `.recv().await` so test failures surface immediately.
    pub async fn recv_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<Option<RexData>, String> {
        match tokio::time::timeout(timeout, self.rx.recv()).await {
            Ok(Some(data)) => Ok(Some(data)),
            Ok(None) => Err("client rx channel closed".to_string()),
            Err(_) => Ok(None),
        }
    }

    pub async fn send(&self, cmd: RexCommand, title: &str, data: &[u8]) -> Result<()> {
        let mut d = RexData::new(cmd, title, data);
        self.client.send_data(&mut d).await
    }

    /// Unbounded wait — a connection that never opens will hang forever.
    /// Prefer `wait_connected_with_timeout(d)`.
    pub async fn wait_connected(&self) {
        while self.client.get_connection_state() != ConnectionState::Connected {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    }

    /// Bounded wait for TCP-connected state. Returns true on success.
    pub async fn wait_connected_with_timeout(&self, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if self.client.get_connection_state() == ConnectionState::Connected {
                return true;
            }
            if tokio::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    }

    /// Wait for the server's `LoginReturn` (title registered server-side).
    /// `wait_connected_with_timeout` only checks TCP state; this waits for
    /// the actual login completion that makes the title routable.
    pub async fn wait_logged_in(&self, timeout: std::time::Duration) -> bool {
        match tokio::time::timeout(timeout, self.logged_in.notified()).await {
            Ok(()) => true,
            Err(_) => false,
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
    logged_in: Arc<Notify>,
}

#[async_trait::async_trait]
impl RexClientHandlerTrait for TestClientHandler {
    async fn login_ok(&self, client: Arc<RexClientInner>, _data: RexData) -> Result<()> {
        info!(
            "login ok, client id: [{:032X}], title: [{}]",
            client.id(),
            client.title_str()
        );
        self.logged_in.notify_waiters();
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
    services: Arc<Services>,
    config: RexSystemConfig,
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
    /// Resolved observability admin address (e.g. `/metrics`).
    /// Returns `None` if no server has been started yet — the bind
    /// happens lazily inside `open_server`.
    pub fn admin_addr(&self) -> Option<SocketAddr> {
        *self.services.admin_addr.lock()
    }

    /// Borrow the underlying `Services` bundle. Use sparingly — direct
    /// access bypasses the factory's lifecycle and exists only for
    /// integration tests that need to call APIs the factory does not
    /// wrap (e.g. `Services::add_client` for restart-restore coverage).
    pub fn services(&self) -> &Arc<Services> {
        &self.services
    }

    /// Snapshot the `RexSystemConfig` this env was built with. Used by
    /// `restart()` to rebuild a fresh `Services` against the same config.
    pub fn config(&self) -> &RexSystemConfig {
        &self.config
    }
}

impl TestEnv {
    pub async fn new() -> Self {
        let mut config = RexSystemConfig::from_id("test-system");
        config.ack_enabled = false;
        Self::from_config(config).await
    }

    /// Create a new TestEnv with ACK enabled
    pub async fn new_with_ack() -> Self {
        let mut config = RexSystemConfig::from_id("test-system");
        config.ack_enabled = true;
        config.ack_timeout = 5000;
        Self::from_config(config).await
    }

    /// Build a TestEnv with persistence enabled at `path` (so the
    /// caller controls the sled Db location — needed by restart tests
    /// that boot the server twice against the same directory).
    /// Sets `check_interval = 1` so the background Janitor wakes
    /// within 1s of `shutdown.signal()`; without this, the Janitor
    /// holds `Arc<Services>` alive for up to 15s, which in turn
    /// holds the sled::Db file lock — and env2's `sled::open` then
    /// fails with WouldBlock.
    pub async fn with_persistence_path(path: std::path::PathBuf) -> Self {
        let mut config = RexSystemConfig::from_id("test-system");
        config.persistence_enabled = true;
        config.persistence_path = path.to_string_lossy().to_string();
        config.check_interval = 1;
        Self::from_config(config).await
    }

    /// Shared construction: config in, Services + base port + observability
    /// out. Always asks the kernel for an ephemeral observability port
    /// (port 0) so parallel tests never collide on 9090. Random base /
    /// cluster ports avoid cross-test TCP collisions.
    async fn from_config(mut config: RexSystemConfig) -> Self {
        let _ = tracing_subscriber::fmt::try_init();
        let base_port = 28800 + (rand::random::<u16>() % 1000);
        let cluster_port = 38800 + (rand::random::<u16>() % 1000);
        config.observability = ObservabilityConfig {
            admin_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            admin_token: None,
            tracing_format: rex_observability::tracing_setup::TracingFormat::Pretty,
            single_node_cluster_ok: true,
            admin_metrics_token: None,
        };
        let shutdown = Shutdown::new();
        let ack_enabled = config.ack_enabled;
        let services = build_services(config.clone(), shutdown, None).await;
        Self {
            services,
            config,
            servers: HashMap::new(),
            base_port,
            ack_enabled,
            cluster_port_counter: cluster_port,
            port_counter: 0,
            server_addrs: HashMap::new(),
        }
    }

    /// Close every running server, flush the state store to disk,
    /// signal the shutdown, drop the current services bundle, and
    /// rebuild a fresh TestEnv against the same `RexSystemConfig`.
    /// Returns the new env; the old self is left in a drained state.
    ///
    /// Use this for tests that need to simulate a process restart
    /// (e.g. persistence restore coverage): the new env reads the
    /// sled Db at the same path the old one wrote.
    pub async fn restart(mut self) -> Result<Self> {
        for (_proto, server) in self.servers.drain() {
            server.close().await;
        }
        // sled's default flush is async; force it before the next open
        // of the same path so the new env's load_all() sees the writes.
        self.services.state_store.flush().await;
        self.services.shutdown.signal();
        drop(self.services);
        // Brief sleep so the sled file lock is released before the
        // fresh `build_services` call opens the same path.
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;

        let base_port = self.base_port;
        let cluster_port_counter = self.cluster_port_counter;
        let port_counter = self.port_counter;
        let ack_enabled = self.ack_enabled;
        let server_addrs = self.server_addrs.clone();
        let mut new_env = Self::from_config(self.config).await;
        new_env.base_port = base_port;
        new_env.cluster_port_counter = cluster_port_counter;
        new_env.port_counter = port_counter;
        new_env.ack_enabled = ack_enabled;
        new_env.server_addrs = server_addrs;
        Ok(new_env)
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
        let server = open_server(self.services.clone(), cfg).await?;
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
        let server = open_server(self.services.clone(), cfg).await?;
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

        let server = open_server(self.services.clone(), cfg).await?;
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
        let logged_in = Arc::new(Notify::new());
        let handler = Arc::new(TestClientHandler {
            tx,
            logged_in: logged_in.clone(),
        });
        let cfg = RexClientConfig::new(proto, server_addr, title, handler);
        let client = open_client(cfg).await?;
        Ok(TestClient::new(client, rx, logged_in))
    }

    /// 创建连接到指定服务器地址的 client
    pub async fn create_client_to_addr(
        &mut self,
        server_addr: SocketAddr,
        title: &str,
    ) -> Result<TestClient> {
        let (tx, rx) = channel(100);
        let logged_in = Arc::new(Notify::new());
        let handler = Arc::new(TestClientHandler {
            tx,
            logged_in: logged_in.clone(),
        });
        let cfg = RexClientConfig::new(Protocol::Tcp, server_addr, title, handler);
        let client = open_client(cfg).await?;
        Ok(TestClient::new(client, rx, logged_in))
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
        let logged_in = Arc::new(Notify::new());
        let handler = Arc::new(TestClientHandler {
            tx,
            logged_in: logged_in.clone(),
        });
        let cfg = RexClientConfig::new(proto, server_addr, title, handler);
        let mut cfg = cfg;
        cfg.ack_enabled = self.ack_enabled;
        cfg.ack_timeout_ms = 5000;
        let client = open_client(cfg).await?;
        Ok(TestClient::new(client, rx, logged_in))
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
        self.services.shutdown.signal();
    }
}
