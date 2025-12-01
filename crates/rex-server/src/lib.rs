pub mod aggregate;
mod config;
pub mod handler;
mod server;
pub mod system;
pub mod transport;

pub use crate::config::RexServerConfig;
pub use crate::transport::{QuicServer, TcpServer, WebSocketServer};
pub use aggregate::*;
pub use server::RexServerTrait;
pub use system::*;

use std::sync::Arc;

use anyhow::Result;

use rex_core::Protocol;

pub async fn open_server(
    system: Arc<RexSystem>,
    server_config: RexServerConfig,
) -> Result<Arc<dyn RexServerTrait>> {
    match server_config.protocol {
        Protocol::Tcp => TcpServer::open(system, server_config).await,
        Protocol::Quic => QuicServer::open(system, server_config).await,
        Protocol::WebSocket => WebSocketServer::open(system, server_config).await,
    }
}
