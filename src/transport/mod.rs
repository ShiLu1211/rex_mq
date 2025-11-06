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

impl Protocol {
    pub fn from(s: &str) -> Option<Self> {
        match s {
            "tcp" => Some(Protocol::Tcp),
            "quic" => Some(Protocol::Quic),
            "websocket" => Some(Protocol::WebSocket),
            _ => None,
        }
    }
}
