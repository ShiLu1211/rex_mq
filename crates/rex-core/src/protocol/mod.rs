mod data;
mod types;

pub use data::{RexData, RexHead};
pub use types::{RetCode, RexCommand};

use serde::{Deserialize, Serialize};
use strum_macros::EnumIter;

#[derive(Debug, Clone, Copy, EnumIter, Serialize, Deserialize, Eq, Hash, PartialEq)]
#[serde(rename_all = "lowercase")]
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
