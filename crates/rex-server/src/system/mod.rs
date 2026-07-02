// src/system/mod.rs
// Re-exports surface ports added in commits 2/3/5/6/7; consumers land in commit 8.
#![allow(unused_imports)]

mod ack;
mod client_registry;
mod cluster_port;
mod config;
mod janitor;
mod offline;
mod registry;
mod services;
mod shutdown;

pub use ack::{AckTracker, AckTrackerImpl, PendingAckInfo};
pub use client_registry::{ClientRegistry, ClientRegistryImpl};
pub use cluster_port::ClusterPort;
pub use config::RexSystemConfig;
pub use janitor::Janitor;
pub use offline::{NoopOfflineBuffer, OfflineBuffer, SledOfflineBuffer};
pub use registry::RexSystem;
pub use services::Services;
pub use shutdown::Shutdown;
