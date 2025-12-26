use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::{Result, bail};
use bytes::BytesMut;
use rex_core::{
    RexClientInner, RexCommand, RexData, RexDataRef, RexFrame, RexFramer, RexSender, WriteCommand,
    utils::{new_uuid, now_secs},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpSocket, tcp::OwnedReadHalf},
    sync::broadcast,
    time::sleep,
};
use tracing::{debug, info, warn};

use crate::{ConnectionState, RexClientConfig, RexClientTrait};

pub struct TcpClient {
    // connection
    client: arc_swap::ArcSwap<Option<Arc<RexClientInner>>>,
    connection_state: AtomicU8,
    // config
    config: RexClientConfig,
    // state management
    shutdown_tx: broadcast::Sender<()>,
    last_heartbeat: AtomicU64,

    worker_tx: kanal::AsyncSender<RexFrame>,
}

#[async_trait::async_trait]
impl RexClientTrait for TcpClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()> {
        let state = self.get_connection_state();

        if state != ConnectionState::Connected {
            bail!("Client not connected (state: {:?})", state);
        }

        if let Some(client) = self.get_client() {
            self.send_data_with_client(&client, data).await
        } else {
            bail!("No active TCP connection")
        }
    }

    async fn close(&self) {
        info!("Shutting down TcpClient...");

        // 发送关闭信号
        let _ = self.shutdown_tx.send(());

        // 关闭客户端连接
        if let Some(client) = self.get_client()
            && let Err(e) = client.close().await
        {
            warn!("Error closing client connection: {}", e);
        }

        // 更新状态
        self.set_connection_state(ConnectionState::Disconnected);

        info!("TcpClient shutdown complete");
    }

    fn get_connection_state(&self) -> ConnectionState {
        self.connection_state.load(Ordering::Relaxed).into()
    }
}

impl TcpClient {
    pub async fn open(config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
        let (worker_tx, worker_rx) = kanal::bounded_async(8192);

        let (shutdown_tx, _) = broadcast::channel(4);
        let client = Arc::new(Self {
            client: arc_swap::ArcSwap::from_pointee(None),
            connection_state: AtomicU8::new(ConnectionState::Disconnected as u8),
            config,
            shutdown_tx,
            last_heartbeat: AtomicU64::new(now_secs()),
            worker_tx,
        });

        // Worker 任务
        tokio::spawn({
            let client = client.clone();
            async move {
                client.worker_task(worker_rx).await;
            }
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

    // 立即处理每条消息，无延迟
    async fn worker_task(self: &Arc<Self>, rx: kanal::AsyncReceiver<RexFrame>) {
        while let Ok(frame) = rx.recv().await {
            let msg = RexData::as_archived(&frame.payload);
            self.handle_archieve_data(&frame.peer, msg).await;
        }
    }

    #[inline(always)]
    fn set_connection_state(&self, state: ConnectionState) {
        self.connection_state.store(state as u8, Ordering::Relaxed);
    }

    // 无锁快速客户端访问
    #[inline(always)]
    fn get_client(&self) -> Option<Arc<RexClientInner>> {
        self.client.load().as_ref().as_ref().cloned()
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
        let (reader, mut writer) = stream.into_split();

        let (tx, rx) = kanal::bounded_async(10000);

        // Writer loop
        tokio::spawn(async move {
            debug!("Writer loop started for {}", local_addr);

            while let Ok(cmd) = rx.recv().await {
                match cmd {
                    WriteCommand::Data(buf) => {
                        if let Err(e) = writer.write_all(&buf).await {
                            warn!("Write error to {}: {}, stopping writer loop", local_addr, e);
                            break;
                        }
                    }
                    WriteCommand::Close => {
                        debug!("Close command received for {}", local_addr);
                        let _ = writer.shutdown().await;
                        break;
                    }
                }
            }
            debug!("Writer loop ended for {}", local_addr);
        });

        let sender = Arc::new(RexSender::new(tx));

        // 使用 ArcSwap 更新客户端（无锁）
        let new_client = Arc::new(RexClientInner::new(
            new_uuid(),
            local_addr,
            &self.config.title().await,
            sender.clone(),
        ));
        self.client.store(Arc::new(Some(new_client)));

        // 启动数据接收任务
        tokio::spawn({
            let this = self.clone();
            let mut shutdown_rx = this.shutdown_tx.subscribe();
            async move {
                tokio::select! {
                    _ = this.receive_data_task(reader) => {
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

    async fn receive_data_task(&self, mut reader: OwnedReadHalf) {
        let mut buffer = BytesMut::with_capacity(self.config.max_buffer_size);
        let mut framer = RexFramer::new(self.config.max_buffer_size);

        let Some(client) = self.get_client() else {
            return;
        };

        loop {
            match reader.read_buf(&mut buffer).await {
                Ok(0) => {
                    info!("Peer closed connection gracefully");
                    break;
                }
                Ok(_) => loop {
                    match framer.try_next_frame(&mut buffer) {
                        Ok(Some(payload)) => {
                            let frame = RexFrame {
                                peer: client.clone(),
                                payload,
                            };
                            if let Err(e) = self.worker_tx.send(frame).await {
                                warn!("Failed to send frame to worker: {}", e);
                            }
                        }
                        Ok(None) => break,
                        Err(e) => {
                            warn!("Framing error: {}", e);
                            buffer.clear();
                            break;
                        }
                    }
                },
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

            let current_state = self.get_connection_state();

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

    async fn heartbeat_task(&self) {
        let heartbeat_interval = Duration::from_secs(15);

        loop {
            sleep(heartbeat_interval).await;

            if self.get_connection_state() != ConnectionState::Connected {
                continue;
            }

            let Some(client) = self.get_client() else {
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

    async fn login(&self) -> Result<()> {
        if let Some(client) = self.get_client() {
            let mut data = RexData::builder(RexCommand::Login)
                .data_from_string(self.config.title().await.clone())
                .build();
            self.send_data_with_client(&client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    async fn handle_archieve_data(&self, client: &Arc<RexClientInner>, data: RexDataRef<'_>) {
        debug!("Handling received data: command={:?}", data.command());

        let handler = self.config.client_handler.clone();

        match data.command() {
            RexCommand::LoginReturn => {
                info!("TCP login successful");
                let rex_data = data.deserialize();
                if let Err(e) = handler.login_ok(client.clone(), rex_data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data.data_as_string_lossy();
                client.insert_title(&title);
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
                let rex_data = data.deserialize();
                if let Err(e) = handler.handle(client.clone(), rex_data).await {
                    warn!("Error in message handler: {}", e);
                }
            }
            _ => {
                debug!("Unhandled command: {:?}", data.command());
            }
        }

        // 更新接收时间
        client.update_last_recv();
    }

    async fn send_data_with_client(
        &self,
        client: &Arc<RexClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        let client_id = client.id();
        data.set_source(client_id);
        client.send_buf(&data.serialize()).await?;
        debug!("TCP data sent successfully: command={:?}", data.command());
        Ok(())
    }
}
