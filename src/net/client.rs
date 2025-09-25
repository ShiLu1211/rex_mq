use std::sync::Arc;

use anyhow::Result;

use crate::{RexClientConfig, protocol::RexData};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[async_trait::async_trait]
pub trait RexClient {
    fn new(config: RexClientConfig) -> Result<Arc<Self>>;

    async fn open(self: Arc<Self>) -> Result<Arc<Self>>;

    async fn send_data(&self, data: &mut RexData) -> Result<()>;

    async fn close(&self);

    async fn get_connection_state(&self) -> ConnectionState;
}
