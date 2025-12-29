use anyhow::Result;
use rex_core::RexData;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ConnectionState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Reconnecting = 3,
}

impl From<u8> for ConnectionState {
    #[inline(always)]
    fn from(value: u8) -> Self {
        match value {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Reconnecting,
            _ => ConnectionState::Disconnected,
        }
    }
}

#[async_trait::async_trait]
pub trait RexClientTrait: Send + Sync {
    async fn send_data(&self, data: &mut RexData) -> Result<()>;

    async fn close(&self);

    fn get_connection_state(&self) -> ConnectionState;
}
