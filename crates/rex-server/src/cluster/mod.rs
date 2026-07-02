//! Cluster integration for RexServer.
//!
//! C2 collapsed three wrapper structs (ClusterManager, ClusterIntegration,
//! ServerClusterManager) into one: `ServerClusterManager` owns the route
//! table and implements `ClusterPort`.
//!
//! `ForwardRequest` and `ForwardType` live in `forward.rs`.

pub mod forward;
pub(crate) mod forward_relay;
pub mod server_cluster;

pub use forward::{ForwardRequest, ForwardType};
pub use server_cluster::ServerClusterManager;
