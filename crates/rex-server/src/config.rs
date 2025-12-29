use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

use crate::Protocol;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RexServerConfig {
    pub protocol: Protocol,
    pub bind_addr: SocketAddr,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default = "default_max_buffer")]
    pub max_buffer_size: usize,
    #[serde(default = "default_max_concurrent")]
    pub max_concurrent_handlers: usize,
}

fn default_enabled() -> bool {
    true
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
            max_buffer_size: 8 * 1024 * 1024,
            max_concurrent_handlers: 1000,
        }
    }

    pub fn from_addr(bind_addr: SocketAddr) -> Self {
        Self::new(Protocol::Tcp, bind_addr)
    }
}
