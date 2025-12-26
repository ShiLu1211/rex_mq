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
    fn from(val: u8) -> Self {
        match val {
            0 => Self::Disconnected,
            1 => Self::Connecting,
            2 => Self::Connected,
            3 => Self::Reconnecting,
            _ => Self::Disconnected,
        }
    }
}

#[async_trait::async_trait]
pub trait RexClientTrait: Send + Sync {
    async fn send_data(&self, data: &mut RexData) -> anyhow::Result<()>;

    async fn close(&self);

    fn get_connection_state(&self) -> ConnectionState;
}
