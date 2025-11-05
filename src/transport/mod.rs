// src/transport/mod.rs
pub mod quic;
pub mod tcp;
pub mod websocket;

pub use quic::{QuicClient, QuicSender, QuicServer};
use strum_macros::EnumIter;
pub use tcp::{TcpClient, TcpSender, TcpServer};
pub use websocket::{WebSocketClient, WebSocketSender, WebSocketServer};

#[derive(Debug, Clone, Copy, EnumIter)]
pub enum Protocol {
    Tcp,
    Quic,
    WebSocket,
}
