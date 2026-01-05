use std::sync::{
    Arc,
    atomic::{AtomicU8, AtomicU64, Ordering},
};
use std::time::Duration;

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{RexClientInner, RexCommand, RexData, utils::now_secs};
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
        client.send_buf(&data.serialize()).await?;
        debug!("Data sent successfully: command={:?}", data.command());
        Ok(())
    }

    /// 通用的登录逻辑
    pub async fn login(&self) -> Result<()> {
        if let Some(client) = self.get_client() {
            let mut data = RexData::new(RexCommand::Login, self.config.title().to_string(), vec![]);
            self.send_data_with_client(client, &mut data).await?;
            info!("Login request sent");
        }
        Ok(())
    }

    /// 通用的接收数据处理逻辑
    pub async fn handle_received_data(&self, data_bytes: &mut [u8]) -> Result<()> {
        let Some(client) = self.get_client() else {
            warn!("No client found, cannot handle received data");
            return Ok(());
        };

        let data = RexData::as_archive(data_bytes);
        let command = RexCommand::from_u32(data.header.command.into());
        debug!("Handling received data: command={:?}", command);

        let handler = self.config.client_handler.clone();

        match command {
            RexCommand::LoginReturn => {
                info!("Login successful");
                let data = RexData::from_archive(data)?;
                if let Err(e) = handler.login_ok(client.clone(), data).await {
                    warn!("Error in login_ok handler: {}", e);
                }
            }
            RexCommand::RegTitleReturn => {
                let title = data.title.as_str();
                client.insert_title(title);
                self.config.set_title(&client.title_str());
                info!("Title registered: {}", title);
            }
            RexCommand::DelTitleReturn => {
                let title = data.title.as_str();
                client.remove_title(title);
                self.config.set_title(&client.title_str());
                info!("Title removed: {}", title);
            }
            RexCommand::Title
            | RexCommand::TitleReturn
            | RexCommand::Group
            | RexCommand::GroupReturn
            | RexCommand::Cast
            | RexCommand::CastReturn => {
                let data = RexData::from_archive(data)?;
                if let Err(e) = handler.handle(client.clone(), data).await {
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

            let ping_bytes = RexData::new(RexCommand::Check, "".to_string(), vec![]).serialize();

            if let Err(e) = client.send_buf(&ping_bytes).await {
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
                Ok(Some(mut data_bytes)) => {
                    if let Err(e) = self.handle_received_data(&mut data_bytes).await {
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
