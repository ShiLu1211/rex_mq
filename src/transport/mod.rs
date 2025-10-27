// src/transport/mod.rs
pub mod quic;
pub mod tcp;

pub use quic::{QuicClient, QuicSender, QuicServer};
pub use tcp::{TcpClient, TcpSender, TcpServer};

#[derive(Debug, Clone, Copy)]
pub enum Protocol {
    Tcp,
    Quic,
}
