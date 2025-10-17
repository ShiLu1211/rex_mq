use anyhow::Result;

use crate::protocol::RexData;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[async_trait::async_trait]
pub trait RexClient {
    async fn send_data(&self, data: &mut RexData) -> Result<()>;

    async fn close(&self);

    async fn get_connection_state(&self) -> ConnectionState;
}
