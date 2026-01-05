use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{
    RexClientInner, RexData,
    utils::{force_set_value, new_uuid},
};
use rex_sender::TcpSender;
use tokio::{
    io::AsyncReadExt,
    net::{TcpSocket, tcp::OwnedReadHalf},
    time::sleep,
};
use tracing::{debug, info, warn};

use super::base::ClientBase;
use crate::{ConnectionState, RexClientConfig, RexClientTrait};

pub struct TcpClient {
    base: ClientBase,
}

#[async_trait::async_trait]
impl RexClientTrait for TcpClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = self.base.connection_state();

        if state != ConnectionState::Connected {
            return Err(anyhow::anyhow!("Client not connected (state: {:?})", state));
        }

        if let Some(client) = self.base.get_client() {
            self.base.send_data_with_client(client, data).await
        } else {
            Err(anyhow::anyhow!("No active TCP connection"))
        }
    }

    async fn close(&self) {
        info!("Shutting down TcpClient...");

        let _ = self.base.shutdown_tx.send(());

        if let Some(client) = self.base.get_client()
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        self.base
            .set_connection_state(ConnectionState::Disconnected);
        info!("TcpClient shutdown complete");
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.base.connection_state()
    }
}

impl TcpClient {
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

        info!("Connecting TCP to {}", self.base.config.server_addr);

        let socket = if self.base.config.server_addr.is_ipv4() {
            TcpSocket::new_v4()?
        } else {
            TcpSocket::new_v6()?
        };
        socket.set_nodelay(true)?;

        let stream = socket.connect(self.base.config.server_addr).await?;
        let local_addr = stream.local_addr()?;
        let (rx, tx) = stream.into_split();

        let sender = Arc::new(TcpSender::new(tx));

        // 创建或更新客户端
        {
            if let Some(existing_client) = self.base.client.as_ref() {
                existing_client.set_sender(sender.clone());
            } else {
                let id = new_uuid();
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
                    _ = this.receive_data_task(rx) => {
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

    async fn receive_data_task(self: &Arc<Self>, mut rx: OwnedReadHalf) {
        let mut buffer = BytesMut::with_capacity(self.base.config.max_buffer_size);

        loop {
            match rx.read_buf(&mut buffer).await {
                Ok(0) => {
                    info!("Peer closed connection gracefully");
                    break;
                }
                Ok(n) => {
                    debug!("Buffered read: {} bytes", n);
                    if let Err(e) = self.base.parse_buffer(&mut buffer).await {
                        warn!("Buffer parsing error: {}", e);
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
