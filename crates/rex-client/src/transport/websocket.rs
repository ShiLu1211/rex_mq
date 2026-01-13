use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use futures_util::StreamExt;
use rex_core::{
    RexClientInner, RexData,
    utils::{force_set_value, new_uuid},
};
use rex_sender::WebSocketSender;
use tokio::time::sleep;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use super::base::ClientBase;
use crate::{ConnectionState, RexClientConfig, RexClientTrait};

pub struct WebSocketClient {
    base: ClientBase,
}

#[async_trait::async_trait]
impl RexClientTrait for WebSocketClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = self.base.connection_state();

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.base.get_client() {
            self.base.send_data_with_client(client, data).await
        } else {
            Err(anyhow::anyhow!("No active WebSocket connection"))
        }
    }

    async fn close(&self) {
        info!("Shutting down WebSocketClient...");

        let _ = self.base.shutdown_tx.send(());

        if let Some(client) = self.base.get_client()
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        self.base
            .set_connection_state(ConnectionState::Disconnected);
        info!("WebSocketClient shutdown complete");
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.base.connection_state()
    }
}

impl WebSocketClient {
    pub async fn open(config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
        let (base, _) = ClientBase::new(config);
        let client = Arc::new(Self { base });

        client.connect_with_retry().await?;

        // 启动连接监控任务
        tokio::spawn({
            let this = client.clone();
            let mut shutdown_rx = this.base.shutdown_tx.subscribe();
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
            let mut shutdown_rx = this.base.shutdown_tx.subscribe();
            async move {
                let get_state = {
                    let this = this.clone();
                    move || this.base.connection_state()
                };
                tokio::select! {
                    _ = this.base.heartbeat_task(get_state) => {
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
                        self.base.config.server_addr, attempts
                    );
                    return Ok(());
                }
                Err(e) => {
                    if attempts >= self.base.config.max_reconnect_attempts {
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
        self.base.set_connection_state(ConnectionState::Connecting);

        info!("Connecting WebSocket to {}", self.base.config.server_addr);

        let url = format!("ws://{}", self.base.config.server_addr);
        let (ws_stream, _) = connect_async(&url).await?;

        let (sink, stream) = ws_stream.split();
        let sender = Arc::new(WebSocketSender::new_client(sink));

        // 创建或更新客户端 (使用 force_set_value 替代 RwLock)
        {
            if let Some(existing_client) = self.base.client.as_ref() {
                existing_client.set_sender(sender.clone());
            } else {
                let id = new_uuid();
                let local_addr = "0.0.0.0:0".parse()?;
                let new_client = Arc::new(RexClientInner::new(
                    id,
                    local_addr,
                    self.base.config.title(),
                    sender.clone(),
                ));
                force_set_value(&self.base.client, Some(new_client));
            }
        }

        // 启动数据接收任务
        tokio::spawn({
            let this = Arc::clone(self);
            let mut shutdown_rx = this.base.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.receive_data_task(stream) => {
                        warn!("Data receiving task ended");
                    }
                    _ = shutdown_rx.recv() => {
                        debug!("Data receiving task received shutdown signal");
                    }
                }

                this.base
                    .set_connection_state(ConnectionState::Disconnected);
            }
        });

        self.base.login().await?;
        self.base.set_connection_state(ConnectionState::Connected);

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
        let mut buffer = BytesMut::with_capacity(self.base.config.max_buffer_size);

        loop {
            match stream.next().await {
                Some(Ok(Message::Binary(data))) => {
                    debug!("Buffered read: {} bytes", data.len());

                    buffer.extend_from_slice(&data);

                    // 尝试解析完整的数据包
                    loop {
                        match RexData::try_deserialize(&mut buffer) {
                            Ok(Some(rex_data)) => {
                                debug!("Parsed message: command={:?}", rex_data.command());
                                if let Err(e) = self.base.handle_received_data(rex_data).await {
                                    warn!("Data handling error: {}", e);
                                }
                            }
                            Ok(None) => {
                                // 数据不完整,等待更多数据
                                break;
                            }
                            Err(e) => {
                                warn!("Data parsing error: {}, clearing buffer", e);
                                buffer.clear();
                                break;
                            }
                        }
                    }

                    // 检查缓冲区是否过大
                    if buffer.len() > self.base.config.max_buffer_size {
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

            let current_state = self.base.connection_state();

            match current_state {
                ConnectionState::Disconnected => {
                    info!("Connection lost, attempting to reconnect...");
                    self.base
                        .set_connection_state(ConnectionState::Reconnecting);

                    if let Err(e) = self.connect_with_retry().await {
                        warn!("Reconnection failed: {}", e);
                        self.base
                            .set_connection_state(ConnectionState::Disconnected);
                    }
                }
                ConnectionState::Reconnecting => {
                    // 重连进行中,等待
                }
                ConnectionState::Connecting | ConnectionState::Connected => {
                    // 正常状态,继续监控
                }
            }
        }
    }
}
