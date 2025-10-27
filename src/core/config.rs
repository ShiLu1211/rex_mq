use std::{net::SocketAddr, sync::Arc};

use tokio::sync::RwLock;

use crate::RexClientHandler;

/// Client configuration
pub struct RexClientConfig {
    pub server_addr: SocketAddr,
    pub title: RwLock<String>,
    pub client_handler: Arc<dyn RexClientHandler>,

    pub idle_timeout: u64,
    pub pong_wait: u64,
    pub max_reconnect_attempts: u32,

    pub read_buffer_size: usize,
    pub max_buffer_size: usize,
}

impl RexClientConfig {
    pub fn new(
        server_addr: SocketAddr,
        title: String,
        client_handler: Arc<dyn RexClientHandler>,
    ) -> Self {
        Self {
            server_addr,
            title: RwLock::new(title),
            client_handler,
            idle_timeout: 10,
            pong_wait: 5,
            max_reconnect_attempts: 5,
            read_buffer_size: 8192,
            max_buffer_size: 8 * 1024 * 1024,
        }
    }

    pub async fn title(&self) -> String {
        self.title.read().await.clone()
    }

    pub async fn set_title(&self, title: String) {
        *self.title.write().await = title;
    }
}

pub struct RexServerConfig {
    pub bind_addr: SocketAddr,
    pub read_buffer_size: usize,
    pub max_buffer_size: usize,
    pub max_concurrent_handlers: usize,
}

impl RexServerConfig {
    pub fn from_addr(bind_addr: SocketAddr) -> Self {
        Self {
            bind_addr,
            read_buffer_size: 8192,
            max_buffer_size: 8 * 1024 * 1024,
            max_concurrent_handlers: 100,
        }
    }
}
