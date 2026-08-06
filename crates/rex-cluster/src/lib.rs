//! RexMQ Cluster Module
//!
//! Cluster membership and inter-node message transport for the RexMQ
//! broker. The cross-node wire I/O seam lives on `Forwarder`
//! (see `rex-server::system::forwarder`); this crate provides only
//! the cluster-side primitives the server's cluster manager and
//! forwarder consume.

pub mod hash_ring;
pub mod node;
pub mod route_table;
pub mod transport;
pub mod types;

pub use types::*;
