mod client;
mod config;
mod handler;
mod transport;

use std::sync::Arc;

use anyhow::Result;
use rex_core::Protocol;

pub use client::{ConnectionState, RexClientTrait};
pub use config::RexClientConfig;
pub use handler::RexClientHandlerTrait;
pub use transport::{QuicClient, TcpClient, WebSocketClient};

pub async fn open_client(client_config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
    match client_config.protocol {
        Protocol::Tcp => TcpClient::open(client_config).await,
        Protocol::Quic => QuicClient::open(client_config).await,
        Protocol::WebSocket => WebSocketClient::open(client_config).await,
    }
}
