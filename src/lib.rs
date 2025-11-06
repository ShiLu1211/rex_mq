pub mod aggregate;
pub mod core;
pub mod handler;
pub mod protocol;
pub mod system;
pub mod transport;
pub mod utils;

use std::sync::Arc;

pub use crate::{core::*, system::*, transport::*};

pub async fn open_client(
    client_config: RexClientConfig,
    protocol: Protocol,
) -> anyhow::Result<Arc<dyn RexClient>> {
    match protocol {
        Protocol::Tcp => TcpClient::open(client_config).await,
        Protocol::Quic => QuicClient::open(client_config).await,
        Protocol::WebSocket => WebSocketClient::open(client_config).await,
    }
}

pub async fn open_server(
    system: Arc<RexSystem>,
    server_config: RexServerConfig,
    protocol: Protocol,
) -> anyhow::Result<Arc<dyn RexServer>> {
    match protocol {
        Protocol::Tcp => TcpServer::open(system, server_config).await,
        Protocol::Quic => QuicServer::open(system, server_config).await,
        Protocol::WebSocket => WebSocketServer::open(system, server_config).await,
    }
}
