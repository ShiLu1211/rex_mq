// src/system/mod.rs
mod client_info;
mod config;
mod registry;

pub use client_info::RexClientInner;
pub use config::RexSystemConfig;
pub use registry::RexSystem;
