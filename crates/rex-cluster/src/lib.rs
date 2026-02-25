//! RexMQ Cluster Module
//!
//! A distributed cluster implementation for RexMQ using Leader-Follower
//! architecture with Gossip protocol for node discovery.

pub mod failover;
pub mod forward;
pub mod gossip;
pub mod hash_ring;
pub mod manager;
pub mod node;
pub mod route_table;
pub mod sync;
pub mod transport;
pub mod types;

#[cfg(test)]
mod manager_tests;

pub use types::*;
