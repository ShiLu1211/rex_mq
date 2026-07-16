// src/system/mod.rs

mod ack;
mod client_registry;
pub mod client_state_store;
mod cluster_port;
mod config;
mod forwarder;
mod janitor;
mod mod_adapters;
mod offline;
mod router;
mod services;
mod shutdown;

pub use ack::{AckTracker, AckTrackerImpl, PendingAckInfo};
pub use client_registry::{ClientRegistry, ClientRegistryImpl, ClientSnapshot};
// ClientStateStore is consumed by Services in the next integration task.
#[allow(unused_imports)]
pub use client_state_store::*;
pub use cluster_port::ClusterPort;
pub use config::RexSystemConfig;
// Forwarder lives in the system bag so commit 2 can wire it into
// Services; nothing imports it yet, so allow the unused imports.
#[allow(unused_imports)]
pub use forwarder::{DeliveryOutcome, Forwarder, FwdResult, NetworkForwarder};
pub use janitor::Janitor;
pub use mod_adapters::{ClientCancelAdapter, RegistryObsAdapter};
pub use offline::{NoopOfflineBuffer, OfflineBuffer, SledOfflineBuffer};
pub use router::{ClusterRouter, RoutePlan, Router};
pub use services::Services;
pub use shutdown::Shutdown;
