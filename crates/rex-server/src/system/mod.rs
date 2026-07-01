// src/system/mod.rs
mod config;
mod registry;
mod shutdown;

pub use config::RexSystemConfig;
pub use registry::RexSystem;
pub use shutdown::Shutdown;
