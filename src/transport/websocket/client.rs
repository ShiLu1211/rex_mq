use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use futures_util::StreamExt;
use tokio::{
    sync::{RwLock, broadcast},
    time::sleep,
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use crate::{
    ConnectionState, RexClient, RexClientConfig, RexClientInner,
    protocol::{RexCommand, RexData},
    utils::{new_uuid, now_secs},
};

use super::WebSocketSender;

pub struct WebSocketClient {
    // connection
    client: RwLock<Option<Arc<RexClientInner>>>,
    connection_state: Arc<RwLock<ConnectionState>>,

    // config
    config: RexClientConfig,

    // state management
    shutdown_tx: broadcast::Sender<()>,
    last_heartbeat: AtomicU64,
}

#[async_trait::async_trait]
impl RexClient for WebSocketClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = *self.connection_state.read().await;

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.get_client().await {
            self.send_data_with_client(&client, data).await
        } else {
            Err(anyhow::anyhow!("No active WebSocket connection"))
        }
    }

    async fn close(&self) {
        info!("Shutting down WebSocketClient...");

        // 发送关闭信号
        let _ = self.shutdown_tx.send(());

        // 关闭客户端连接
        if let Some(client) = self.get_client().await
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        // 更新状态
        *self.connection_state.write().await = ConnectionState::Disconnected;

        info!("WebSocketClient shutdown complete");
    }

    async fn get_connection_state(&self) -> ConnectionState {
        *self.connection_state.read().await
    }
}

impl WebSocketClient {
    pub async fn open(config: RexClientConfig) -> Result<Arc<dyn RexClient>> {
        let (shutdown_tx, _) = broadcast::channel(4);

        let client = Arc::new(Self {
            client: RwLock::new(None),
            connection_state: Arc::new(RwLock::new(ConnectionState::Disconnected)),
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
        *self.connection_state.write().await = ConnectionState::Connecting;

        info!("Connecting WebSocket to {}", self.config.server_addr);

        let url = format!("ws://{}", self.config.server_addr);
        let (ws_stream, _) = connect_async(&url).await?;

        let (sink, stream) = ws_stream.split();
        let sender = Arc::new(WebSocketSender::new_client(sink));

        // 创建或更新客户端
        {
            let mut client_guard = self.client.write().await;
            if let Some(existing_client) = client_guard.as_ref() {
                existing_client.set_sender(sender.clone()).await;
            } else {
                let id = new_uuid();
                let local_addr = "0.0.0.0:0".parse()?;
                let new_client = Arc::new(RexClientInner::new(
                    id,
                    local_addr,
                    &self.config.title().await,
                    sender.clone(),
                ));
                *client_guard = Some(new_client);
            }
        }

        // 启动数据接收任务
        tokio::spawn({
            let this = Arc::clone(self);
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.receive_data_task(stream) => {
                        warn!("Data receiving task ended");
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("Data receiving task received shutdown signal");
                    }
                }

                // 标记连接断开
                *this.connection_state.write().await = ConnectionState::Disconnected;
            }
        });

        // 执行登录
        self.login().await?;

        // 更新连接状态
        *self.connection_state.write().await = ConnectionState::Connected;
        self.last_heartbeat.store(now_secs(), Ordering::Relaxed);

        Ok(())
    }

    async fn receive_data_task(
        self: &Arc<Self>,
        mut stream: futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
    ) {
        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);

        loop {
            match stream.next().await {
                Some(Ok(Message::Binary(data))) => {
                    debug!("Buffered read: {} bytes", data.len());

                    buffer.extend_from_slice(&data);

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
                Some(Ok(Message::Close(_))) => {
                    info!("WebSocket connection closed by server");
                    break;
                }
                Some(Ok(Message::Ping(_))) => {
                    // WebSocket 库会自动处理 pong
                    debug!("Received ping from server");
                }
                Some(Ok(_)) => {
                    // 忽略其他类型的消息
                }
                Some(Err(e)) => {
                    warn!("WebSocket read error: {}", e);
                    break;
                }
                None => {
                    info!("WebSocket stream ended");
                    break;
                }
            }
        }
    }

    async fn connection_monitor_task(self: &Arc<Self>) {
        let check_interval = Duration::from_secs(5);

        loop {
            sleep(check_interval).await;

            let current_state = *self.connection_state.read().await;

            match current_state {
                ConnectionState::Disconnected => {
                    info!("Connection lost, attempting to reconnect...");
                    *self.connection_state.write().await = ConnectionState::Reconnecting;

                    if let Err(e) = self.connect_with_retry().await {
                        warn!("Reconnection failed: {}", e);
                        *self.connection_state.write().await = ConnectionState::Disconnected;
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

            if *self.connection_state.read().await != ConnectionState::Connected {
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
                *self.connection_state.write().await = ConnectionState::Disconnected;
                continue;
            }

            // 等待心跳响应
            let before_ping = last_recv;
            sleep(Duration::from_secs(self.config.pong_wait)).await;
            let after_ping = client.last_recv();

            if after_ping <= before_ping {
                warn!("Heartbeat timeout, marking connection as lost");
                *self.connection_state.write().await = ConnectionState::Disconnected;
            } else {
                debug!("Heartbeat successful");
                self.last_heartbeat.store(now_secs(), Ordering::Relaxed);
            }
        }
    }

    async fn login(self: &Arc<Self>) -> Result<()> {
        if let Some(client) = self.get_client().await {
            let mut data = RexData::builder(RexCommand::Login)
                .data_from_string(self.config.title().await.clone())
                .build();
            self.send_data_with_client(&client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    async fn on_data(&self, data: RexData) {
        if let Some(client) = self.get_client().await {
            self.handle_received_data(&client, data).await;
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
                info!("WebSocket login successful");
                if let Err(e) = handler.login_ok(client.clone(), data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data.data_as_string_lossy();
                client.insert_title(title.clone());
                self.config.set_title(client.title_str()).await;
                info!("Title registered: {}", title);
            }
            RexCommand::DelTitleReturn => {
                let title = data.data_as_string_lossy();
                client.remove_title(&title);
                self.config.set_title(client.title_str()).await;
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

    async fn get_client(&self) -> Option<Arc<RexClientInner>> {
        self.client.read().await.clone()
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<RexClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        let client_id = client.id().await;
        data.set_source(client_id);
        client.send_buf(&data.serialize()).await?;
        debug!(
            "WebSocket data sent successfully: command={:?}",
            data.header().command()
        );
        Ok(())
    }
}
