// src/system/mod.rs
// Re-exports surface ports added in commits 2/3/5; consumers land in commit 7+.
#![allow(unused_imports)]

mod ack;
mod client_registry;
mod config;
mod janitor;
mod offline;
mod registry;
mod shutdown;

pub use ack::{AckTracker, AckTrackerImpl, PendingAckInfo};
pub use client_registry::{ClientRegistry, ClientRegistryImpl};
pub use config::RexSystemConfig;
pub use janitor::Janitor;
pub use offline::{NoopOfflineBuffer, OfflineBuffer, SledOfflineBuffer};
pub use registry::RexSystem;
pub use shutdown::Shutdown;
