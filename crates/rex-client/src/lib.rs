mod client;
mod config;
mod handler;
mod sender;
mod transport;

use std::sync::Arc;

use anyhow::Result;
use rex_core::Protocol;

pub use client::{ConnectionState, RexClientInner, RexClientTrait};
pub use config::RexClientConfig;
pub use handler::RexClientHandlerTrait;
pub use sender::RexSenderTrait;
pub use transport::{
    QuicClient, QuicSender, TcpClient, TcpSender, WebSocketClient, WebSocketSender,
};

pub async fn open_client(client_config: RexClientConfig) -> Result<Arc<dyn RexClientTrait>> {
    match client_config.protocol {
        Protocol::Tcp => TcpClient::open(client_config).await,
        Protocol::Quic => QuicClient::open(client_config).await,
        Protocol::WebSocket => WebSocketClient::open(client_config).await,
    }
}
