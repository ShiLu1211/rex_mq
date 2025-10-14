// src/core/mod.rs
pub mod client;
pub mod config;
pub mod error;
pub mod handler;
pub mod sender;
pub mod server;

pub use client::{ConnectionState, RexClient};
pub use config::{RexClientConfig, RexServerConfig};
pub use error::RexError;
pub use handler::RexClientHandler;
pub use sender::RexSender;
pub use server::RexServer;
