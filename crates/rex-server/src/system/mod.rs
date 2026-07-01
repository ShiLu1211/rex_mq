// src/system/mod.rs
#[allow(unused_imports)]
// Re-exports surface the port added in commit 2; consumers land in commit 7+.
mod client_registry;
mod config;
mod registry;
mod shutdown;

pub use client_registry::{ClientRegistry, ClientRegistryImpl};
pub use config::RexSystemConfig;
pub use registry::RexSystem;
pub use shutdown::Shutdown;
