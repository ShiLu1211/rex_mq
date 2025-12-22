use std::{net::SocketAddr, sync::Arc};

use rex_core::Protocol;
use tokio::sync::RwLock;

use crate::RexClientHandlerTrait;

/// Client configuration
#[derive(Clone)]
pub struct RexClientConfig {
    pub protocol: Protocol,
    pub server_addr: SocketAddr,
    pub title: Arc<RwLock<String>>,
    pub client_handler: Arc<dyn RexClientHandlerTrait>,

    pub idle_timeout: u64,
    pub pong_wait: u64,
    pub max_reconnect_attempts: u32,

    pub max_buffer_size: usize,
}

impl RexClientConfig {
    pub fn new(
        protocol: Protocol,
        server_addr: SocketAddr,
        title: String,
        client_handler: Arc<dyn RexClientHandlerTrait>,
    ) -> Self {
        Self {
            protocol,
            server_addr,
            title: Arc::new(RwLock::new(title)),
            client_handler,
            idle_timeout: 10,
            pong_wait: 5,
            max_reconnect_attempts: 5,
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
