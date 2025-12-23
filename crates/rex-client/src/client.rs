use rex_core::RexData;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[async_trait::async_trait]
pub trait RexClientTrait: Send + Sync {
    async fn send_data(&self, data: &mut RexData) -> anyhow::Result<()>;

    async fn close(&self);

    async fn get_connection_state(&self) -> ConnectionState;
}
