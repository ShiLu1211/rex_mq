use std::sync::Arc;

use anyhow::Result;
use bytes::BytesMut;
use rex_core::{RexClientInner, RexData};
use tokio::sync::{Semaphore, broadcast};
use tracing::{debug, warn};

use crate::{RexServerConfig, RexSystem, handler::handle};

/// 服务器基础结构,包含所有通用字段
pub struct ServerBase {
    pub system: Arc<RexSystem>,
    pub config: RexServerConfig,
    pub semaphore: Arc<Semaphore>,
    pub shutdown_tx: Arc<broadcast::Sender<()>>,
}

impl ServerBase {
    pub fn new(system: Arc<RexSystem>, config: RexServerConfig) -> (Self, broadcast::Receiver<()>) {
        let semaphore = Arc::new(Semaphore::new(config.max_concurrent_handlers));
        let (shutdown_tx, shutdown_rx) = broadcast::channel(2 + config.max_concurrent_handlers);

        let base = Self {
            system,
            config,
            semaphore,
            shutdown_tx: Arc::new(shutdown_tx),
        };

        (base, shutdown_rx)
    }

    /// 通用的数据解析和处理逻辑
    pub async fn parse_and_handle_buffer(
        &self,
        peer: &Arc<RexClientInner>,
        buffer: &mut BytesMut,
    ) -> Result<()> {
        let peer_addr = peer.local_addr();

        loop {
            match RexData::try_deserialize(buffer) {
                Ok(Some(mut rex_data)) => {
                    debug!(
                        "Received data from {}: command={:?}",
                        peer_addr,
                        rex_data.command(),
                    );

                    if let Err(e) = handle(&self.system, peer, &mut rex_data).await {
                        warn!("Error handling data from {}: {}", peer_addr, e);
                    }

                    peer.update_last_recv();
                }
                Ok(None) => {
                    // 数据不完整,等待更多数据
                    break;
                }
                Err(e) => {
                    warn!(
                        "Error parsing data from {}: {}, clearing buffer",
                        peer_addr, e
                    );
                    buffer.clear();
                    break;
                }
            }
        }

        // 检查缓冲区大小,防止内存泄漏
        if buffer.len() > self.config.max_buffer_size {
            warn!(
                "Buffer too large for connection {} ({}KB), clearing",
                peer_addr,
                buffer.len() / 1024
            );
            buffer.clear();
        }

        Ok(())
    }

    /// 通用的关闭逻辑
    pub fn send_shutdown_signal(&self) {
        if let Err(e) = self.shutdown_tx.send(()) {
            warn!("Error sending shutdown signal: {}", e);
        }
    }

    /// 获取一个连接许可
    pub async fn acquire_connection_permit(&self) -> Result<tokio::sync::OwnedSemaphorePermit> {
        self.semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to acquire connection permit: {}", e))
    }
}
