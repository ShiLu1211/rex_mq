use serde::{Deserialize, Serialize};
use std::{net::SocketAddr, sync::Arc};
use tokio::sync::RwLock;

use crate::{Protocol, RexClientHandler};

/// Client configuration
#[derive(Clone)]
pub struct RexClientConfig {
    pub protocol: Protocol,
    pub server_addr: SocketAddr,
    pub title: Arc<RwLock<String>>,
    pub client_handler: Arc<dyn RexClientHandler>,

    pub idle_timeout: u64,
    pub pong_wait: u64,
    pub max_reconnect_attempts: u32,

    pub read_buffer_size: usize,
    pub max_buffer_size: usize,
}

impl RexClientConfig {
    pub fn new(
        protocol: Protocol,
        server_addr: SocketAddr,
        title: String,
        client_handler: Arc<dyn RexClientHandler>,
    ) -> Self {
        Self {
            protocol,
            server_addr,
            title: Arc::new(RwLock::new(title)),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RexServerConfig {
    pub protocol: Protocol,
    pub bind_addr: SocketAddr,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_read_buffer")]
    pub read_buffer_size: usize,
    #[serde(default = "default_max_buffer")]
    pub max_buffer_size: usize,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_handlers: usize,
}

fn default_enabled() -> bool {
    true
}
fn default_read_buffer() -> usize {
    8192
}
fn default_max_buffer() -> usize {
    8 * 1024 * 1024
}
fn default_max_concurrent() -> usize {
    1000
}

impl RexServerConfig {
    pub fn new(protocol: Protocol, bind_addr: SocketAddr) -> Self {
        Self {
            protocol,
            bind_addr,
            enabled: true,
            read_buffer_size: 8192,
            max_buffer_size: 8 * 1024 * 1024,
            max_concurrent_handlers: 1000,
        }
    }

    pub fn from_addr(bind_addr: SocketAddr) -> Self {
        Self::new(Protocol::Tcp, bind_addr)
    }
}
