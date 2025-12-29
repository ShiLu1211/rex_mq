use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{
    RexClientInner, RexCommand, RexData,
    utils::{force_set_value, new_uuid, now_secs},
};
use rex_sender::TcpSender;
use tokio::{
    io::AsyncReadExt,
    net::{TcpSocket, tcp::OwnedReadHalf},
    sync::broadcast,
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::{ConnectionState, RexClientConfig, RexClientTrait};

pub struct TcpClient {
    // connection
    client: Option<Arc<RexClientInner>>,
    connection_state: AtomicU8,

    // config
    config: RexClientConfig,

    // state management
    shutdown_tx: broadcast::Sender<()>,
    last_heartbeat: AtomicU64,
}

#[async_trait::async_trait]
impl RexClientTrait for TcpClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = self.connection_state();

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.get_client().await {
            self.send_data_with_client(client, data).await
        } else {
            Err(anyhow::anyhow!("No active TCP connection"))
        }
    }

    async fn close(&self) {
        info!("Shutting down TcpClient...");

        // 发送关闭信号
        let _ = self.shutdown_tx.send(());

        // 关闭客户端连接
        if let Some(client) = self.get_client().await
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        // 更新状态
        self.set_connection_state(ConnectionState::Disconnected);

        info!("TcpClient shutdown complete");
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.connection_state()
    }
}

impl TcpClient {
    pub async fn open(config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
        let (shutdown_tx, _) = broadcast::channel(4);
        let client = Arc::new(Self {
            client: None,
            connection_state: AtomicU8::new(ConnectionState::Disconnected as u8),
            config,
            shutdown_tx,
            last_heartbeat: AtomicU64::new(now_secs()),
        });

        // 初始连接
        client.connect_with_retry().await?;

        // 启动连接监控任务
        tokio::spawn({
            let this = client.clone();
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.connection_monitor_task() => {
                        info!("Connection monitor task completed");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Connection monitor task received shutdown signal");
                    }
                }
            }
        });

        // 启动心跳任务
        tokio::spawn({
            let this = client.clone();
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.heartbeat_task() => {
                        info!("Heartbeat task completed");
                    }
                    _ = shutdown_rx.recv() => {
                        info!("Heartbeat task received shutdown signal");
                    }
                }
            }
        });

        Ok(client)
    }

    #[inline(always)]
    fn connection_state(&self) -> ConnectionState {
        self.connection_state.load(Ordering::Relaxed).into()
    }

    #[inline(always)]
    fn set_connection_state(&self, state: ConnectionState) {
        self.connection_state.store(state as u8, Ordering::Relaxed);
    }

    async fn connect_with_retry(self: &Arc<Self>) -> Result<()> {
        let mut attempts = 0;
        let mut backoff = 1;

        loop {
            attempts += 1;

            match self.connect().await {
                Ok(_) => {
                    info!(
                        "Connected to {} after {} attempts",
                        self.config.server_addr, attempts
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= self.config.max_reconnect_attempts {
                        warn!("Failed to connect after {} attempts: {}", attempts, e);
                        return Err(e);
                    }

                    warn!(
                        "Connection attempt {} failed: {}, retrying in {}s",
                        attempts, e, backoff
                    );
                    sleep(Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(60);
                }
            }
        }
    }

    async fn connect(self: &Arc<Self>) -> Result<()> {
        self.set_connection_state(ConnectionState::Connecting);

        info!("Connecting TCP to {}", self.config.server_addr);

        let socket = if self.config.server_addr.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        socket.set_nodelay(true)?;

        let stream = socket.connect(self.config.server_addr).await?;
        let local_addr = stream.local_addr()?;
        let (rx, tx) = stream.into_split();

        let sender = Arc::new(TcpSender::new(tx));

        // 创建或更新客户端
        {
            if let Some(existing_client) = self.client.as_ref() {
                existing_client.set_sender(sender.clone());
            } else {
                let id = new_uuid();
                let new_client = Arc::new(RexClientInner::new(
                    id,
                    local_addr,
                    self.config.title(),
                    sender.clone(),
                ));
                force_set_value(&self.client, Some(new_client));
            }
        }

        // 启动数据接收任务
        tokio::spawn({
            let this = Arc::clone(self);
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.receive_data_task(rx) => {
                        warn!("Data receiving task ended");
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("Data receiving task received shutdown signal");
                    }
                }

                // 标记连接断开
                this.set_connection_state(ConnectionState::Disconnected);
            }
        });

        // 执行登录
        self.login().await?;

        // 更新连接状态
        self.set_connection_state(ConnectionState::Connected);
        self.last_heartbeat.store(now_secs(), Ordering::Relaxed);

        Ok(())
    }

    async fn receive_data_task(self: &Arc<Self>, mut rx: OwnedReadHalf) {
        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut total_read = 0;

        loop {
            match rx.read_buf(&mut buffer).await {
                Ok(0) => {
                    info!("Peer closed connection gracefully");
                    break;
                }
                Ok(n) => {
                    total_read += n;
                    debug!("Buffered read: {} bytes (total: {})", n, total_read);

                    // 尝试解析完整的数据包
                    loop {
                        match RexData::try_deserialize(&mut buffer) {
                            Ok(Some(data)) => {
                                debug!("Parsed message: command={:?}", data.header().command(),);
                                self.on_data(data).await;
                            }
                            Ok(None) => {
                                // 数据不完整，等待更多数据
                                break;
                            }
                            Err(e) => {
                                // 错误处理
                                warn!("Data parsing error: {}, clearing buffer", e);
                                buffer.clear();
                                break;
                            }
                        }
                    }

                    // 检查缓冲区是否过大
                    if buffer.len() > self.config.max_buffer_size {
                        warn!(
                            "Read buffer too large ({}KB), clearing",
                            buffer.len() / 1024
                        );
                        buffer.clear();
                    }
                }
                Err(e) => {
                    warn!("TCP read error: {}", e);
                    break;
                }
            }
        }
    }

    async fn connection_monitor_task(self: &Arc<Self>) {
        let check_interval = Duration::from_secs(5);

        loop {
            sleep(check_interval).await;

            let current_state = self.connection_state();

            match current_state {
                ConnectionState::Disconnected => {
                    info!("Connection lost, attempting to reconnect...");
                    self.set_connection_state(ConnectionState::Reconnecting);

                    if let Err(e) = self.connect_with_retry().await {
                        warn!("Reconnection failed: {}", e);
                        self.set_connection_state(ConnectionState::Disconnected);
                    }
                }
                ConnectionState::Reconnecting => {
                    // 重连进行中，等待
                }
                ConnectionState::Connecting | ConnectionState::Connected => {
                    // 正常状态，继续监控
                }
            }
        }
    }

    async fn heartbeat_task(self: &Arc<Self>) {
        let heartbeat_interval = Duration::from_secs(15);

        loop {
            sleep(heartbeat_interval).await;

            if self.connection_state() != ConnectionState::Connected {
                continue;
            }

            let Some(client) = self.get_client().await else {
                warn!("No client found, cannot send heartbeat");
                continue;
            };

            let last_recv = client.last_recv();
            let idle_time = now_secs().saturating_sub(last_recv);

            if idle_time < self.config.idle_timeout {
                continue;
            }

            // 发送心跳
            debug!("Sending heartbeat (idle: {}s)", idle_time);
            let ping = RexData::builder(RexCommand::Check).build().serialize();

            if let Err(e) = client.send_buf(&ping).await {
                warn!("Heartbeat send failed: {}", e);
                self.set_connection_state(ConnectionState::Disconnected);
                continue;
            }

            // 等待心跳响应
            let before_ping = last_recv;
            sleep(Duration::from_secs(self.config.pong_wait)).await;
            let after_ping = client.last_recv();

            if after_ping <= before_ping {
                warn!("Heartbeat timeout, marking connection as lost");
                self.set_connection_state(ConnectionState::Disconnected);
            } else {
                debug!("Heartbeat successful");
                self.last_heartbeat.store(now_secs(), Ordering::Relaxed);
            }
        }
    }

    async fn login(self: &Arc<Self>) -> Result<()> {
        if let Some(client) = self.get_client().await {
            let mut data = RexData::builder(RexCommand::Login)
                .data_from_string(self.config.title())
                .build();
            self.send_data_with_client(client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    async fn on_data(&self, data: RexData) {
        if let Some(client) = self.get_client().await {
            self.handle_received_data(client, data).await;
        }
    }

    async fn handle_received_data(&self, client: &Arc<RexClientInner>, data: RexData) {
        debug!(
            "Handling received data: command={:?}",
            data.header().command()
        );

        let handler = self.config.client_handler.clone();

        match data.header().command() {
            RexCommand::LoginReturn => {
                info!("TCP login successful");
                if let Err(e) = handler.login_ok(client.clone(), data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data.data_as_string_lossy();
                client.insert_title(&title);
                self.config.set_title(&client.title_str());
                info!("Title registered: {}", title);
            }
            RexCommand::DelTitleReturn => {
                let title = data.data_as_string_lossy();
                client.remove_title(&title);
                self.config.set_title(&client.title_str());
                info!("Title removed: {}", title);
            }
            RexCommand::Title
            | RexCommand::TitleReturn
            | RexCommand::Group
            | RexCommand::GroupReturn
            | RexCommand::Cast
            | RexCommand::CastReturn => {
                if let Err(e) = handler.handle(client.clone(), data).await {
                    warn!("Error in message handler: {}", e);
                }
            }
            _ => {
                debug!("Unhandled command: {:?}", data.header().command());
            }
        }

        // 更新接收时间
        client.update_last_recv();
    }

    async fn get_client(&self) -> &Option<Arc<RexClientInner>> {
        &self.client
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<RexClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        let client_id = client.id();
        data.set_source(client_id);
        client.send_buf(&data.serialize()).await?;
        debug!(
            "TCP data sent successfully: command={:?}",
            data.header().command()
        );
        Ok(())
    }
}
