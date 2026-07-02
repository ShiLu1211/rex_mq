//! Forward request types — shared between handler/title.rs and
//! cluster/server_cluster.rs. Moved here from cluster.rs when that file
//! was deleted in C2.

/// Request to forward a message to another node.
#[derive(Debug, Clone)]
pub struct ForwardRequest {
    /// Original source client ID
    pub source_client_id: u128,
    /// Target client ID (0 if not specific)
    pub target_client_id: u128,
    /// Message title
    pub title: String,
    /// Message payload
    pub payload: Vec<u8>,
    /// Message type
    pub msg_type: ForwardType,
}

/// Type of forward
#[derive(Debug, Clone, Copy)]
pub enum ForwardType {
    /// Unicast (single target)
    Unicast,
    /// Multicast (one of group)
    Group,
    /// Broadcast (all subscribers)
    Broadcast,
}
