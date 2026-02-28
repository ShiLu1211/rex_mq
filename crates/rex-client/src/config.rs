use std::{net::SocketAddr, sync::Arc};

use rex_core::{Protocol, utils::force_set_value};

use crate::RexClientHandlerTrait;

/// Client configuration
#[derive(Clone)]
pub struct RexClientConfig {
    pub protocol: Protocol,
    pub server_addr: SocketAddr,
    pub title: String,
    pub client_handler: Arc<dyn RexClientHandlerTrait>,

    pub idle_timeout: u64,
    pub pong_wait: u64,
    pub max_reconnect_attempts: u32,

    pub max_buffer_size: usize,

    /// Whether to request ACK for sent messages
    pub ack_enabled: bool,
    /// ACK timeout in milliseconds
    pub ack_timeout_ms: u64,
}

impl RexClientConfig {
    pub fn new(
        protocol: Protocol,
        server_addr: SocketAddr,
        title: &str,
        client_handler: Arc<dyn RexClientHandlerTrait>,
    ) -> Self {
        Self {
            protocol,
            server_addr,
            title: title.to_string(),
            client_handler,
            idle_timeout: 10,
            pong_wait: 5,
            max_reconnect_attempts: 5,
            max_buffer_size: 8 * 1024 * 1024,
            ack_enabled: false,
            ack_timeout_ms: 5000,
        }
    }

    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    pub fn set_title(&self, title: &str) {
        force_set_value(&self.title, title.to_string());
    }
}
