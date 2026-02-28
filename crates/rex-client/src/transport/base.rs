use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use dashmap::DashMap;
use rex_core::{AckData, RexClientInner, RexCommand, RexData, utils::now_secs};
use tokio::{sync::broadcast, time::sleep};
use tracing::{debug, info, warn};

use crate::{ConnectionState, RexClientConfig};

/// 基础客户端结构,包含所有通用字段和方法
pub struct ClientBase {
    pub client: Option<Arc<RexClientInner>>,
    pub connection_state: AtomicU8,
    pub config: RexClientConfig,
    pub shutdown_tx: broadcast::Sender<()>,
    pub last_heartbeat: AtomicU64,
    /// Pending ACKs for sent messages (message_id -> sender info)
    pub pending_acks: DashMap<u64, PendingAckInfo>,
}

/// Information about a pending ACK
pub struct PendingAckInfo {
    pub timestamp: u64,
    pub title: String,
}

impl ClientBase {
    pub fn new(config: RexClientConfig) -> (Self, broadcast::Receiver<()>) {
        let (shutdown_tx, shutdown_rx) = broadcast::channel(4);

        let base = Self {
            client: None,
            connection_state: AtomicU8::new(ConnectionState::Disconnected as u8),
            config,
            shutdown_tx,
            last_heartbeat: AtomicU64::new(now_secs()),
            pending_acks: DashMap::new(),
        };

        (base, shutdown_rx)
    }

    #[inline(always)]
    pub fn connection_state(&self) -> ConnectionState {
        self.connection_state.load(Ordering::Relaxed).into()
    }

    #[inline(always)]
    pub fn set_connection_state(&self, state: ConnectionState) {
        self.connection_state.store(state as u8, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn get_client(&self) -> &Option<Arc<RexClientInner>> {
        &self.client
    }

    /// 通用的数据发送逻辑
    pub async fn send_data_with_client(
        &self,
        client: &Arc<RexClientInner>,
        data: &mut RexData,
    ) -> Result<()> {
        let client_id = client.id();
        data.set_source(client_id);

        // If ACK is enabled and this is a message that expects ACK, register pending ACK
        if self.config.ack_enabled {
            match data.command() {
                RexCommand::Title | RexCommand::Group | RexCommand::Cast => {
                    // Generate message ID for ACK tracking
                    let message_id = fastrand::u64(..);
                    data.set_message_id(message_id);

                    // Register pending ACK
                    self.register_pending_ack(message_id, data.title().to_string());
                    debug!("Registered pending ACK for message {}", message_id);
                }
                _ => {}
            }
        }

        client.send_buf(data.pack_ref()).await?;
        debug!("Data sent successfully: command={:?}", data.command());
        Ok(())
    }

    /// 通用的登录逻辑
    pub async fn login(&self) -> Result<()> {
        if let Some(client) = self.get_client() {
            let mut data = RexData::new(RexCommand::Login, self.config.title(), &[]);
            self.send_data_with_client(client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    /// 通用的接收数据处理逻辑
    pub async fn handle_received_data(&self, rex_data: RexData) -> Result<()> {
        let Some(client) = self.get_client() else {
            warn!("No client found, cannot handle received data");
            return Ok(());
        };

        let command = rex_data.command();
        debug!("Handling received data: command={:?}", command);

        let handler = self.config.client_handler.clone();

        match command {
            RexCommand::LoginReturn => {
                info!("Login successful");
                if let Err(e) = handler.login_ok(client.clone(), rex_data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = rex_data.title();
                client.insert_title(title);
                self.config.set_title(&client.title_str());
                info!("Title registered: {}", title);
            }
            RexCommand::DelTitleReturn => {
                let title = rex_data.title();
                client.remove_title(title);
                self.config.set_title(&client.title_str());
                info!("Title removed: {}", title);
            }
            RexCommand::Title | RexCommand::Group | RexCommand::Cast => {
                // Check if message needs ACK
                let message_id = rex_data.message_id();
                debug!("Received message with message_id: {}", message_id);
                if message_id != 0 {
                    // Send ACK back to server
                    self.send_ack(message_id).await;
                }

                // Pass message to handler
                if let Err(e) = handler.handle(client.clone(), rex_data).await {
                    warn!("Error in message handler: {}", e);
                }
            }
            RexCommand::AckReturn => {
                // Handle ACK return for sent messages
                self.handle_ack_return(&rex_data).await;
                // Also pass to handler so application can receive it
                if let Err(e) = handler.handle(client.clone(), rex_data).await {
                    warn!("Error in ACK handler: {}", e);
                }
            }
            RexCommand::TitleReturn | RexCommand::GroupReturn | RexCommand::CastReturn => {
                if let Err(e) = handler.handle(client.clone(), rex_data).await {
                    warn!("Error in message handler: {}", e);
                }
            }
            _ => {
                debug!("Unhandled command: {:?}", command);
            }
        }

        client.update_last_recv();
        Ok(())
    }

    /// Send ACK to server for a received message
    async fn send_ack(&self, message_id: u64) {
        let Some(client) = self.get_client() else {
            warn!("No client found, cannot send ACK");
            return;
        };

        let ack_data = AckData::new(message_id);
        let rex_data = ack_data.to_rex_data(client.id(), RexCommand::Ack);

        debug!("Sending ACK for message {} to server", message_id);
        if let Err(e) = client.send_buf(rex_data.pack_ref()).await {
            warn!("Failed to send ACK for message {}: {}", message_id, e);
        } else {
            debug!("ACK sent for message {}", message_id);
        }
    }

    /// Handle ACK return from server
    async fn handle_ack_return(&self, rex_data: &RexData) {
        let ack_data = AckData::from_rex_data(rex_data);
        let message_id = ack_data.message_id;
        let retcode = rex_data.retcode();

        // Remove from pending ACKs
        if self.pending_acks.remove(&message_id).is_some() {
            debug!("ACK received for message {}: {:?}", message_id, retcode);
        } else {
            debug!(
                "ACK received for unknown message {}: {:?}",
                message_id, retcode
            );
        }
    }

    /// Register a pending ACK for a sent message
    pub fn register_pending_ack(&self, message_id: u64, title: String) {
        let info = PendingAckInfo {
            timestamp: now_secs(),
            title,
        };
        self.pending_acks.insert(message_id, info);
    }

    /// 通用的心跳任务
    pub async fn heartbeat_task<F>(&self, get_state: F)
    where
        F: Fn() -> ConnectionState + Send + 'static,
    {
        let heartbeat_interval = Duration::from_secs(15);

        loop {
            sleep(heartbeat_interval).await;

            if get_state() != ConnectionState::Connected {
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

            debug!("Sending heartbeat (idle: {}s)", idle_time);

            let ping = RexData::new(RexCommand::Check, "", &[]);

            if let Err(e) = client.send_buf(ping.pack_ref()).await {
                warn!("Heartbeat send failed: {}", e);
                self.set_connection_state(ConnectionState::Disconnected);
                continue;
            }

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

    /// 通用的数据解析逻辑
    pub async fn parse_buffer(&self, buffer: &mut BytesMut) -> Result<()> {
        loop {
            match RexData::try_deserialize(buffer) {
                Ok(Some(rex_data)) => {
                    if let Err(e) = self.handle_received_data(rex_data).await {
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
        if buffer.len() > self.config.max_buffer_size {
            warn!(
                "Read buffer too large ({}KB), clearing",
                buffer.len() / 1024
            );
            buffer.clear();
        }

        Ok(())
    }
}
