//! Delivers forwarded cluster messages to local subscribers.
//!
//! Extracted from `ServerClusterManager` (C1 candidate) so the cluster
//! lifecycle struct stays focused on membership, gossip, and transport.
//! The module is stateless — every call receives `&Services`.

use bytes::BytesMut;
use rex_cluster::types::{ClusterMessage, ForwardAckMessage, ForwardMessage};
use rex_core::RexData;
use tracing::{debug, info, warn};

use crate::Services;

/// Deliver a `ForwardMessage` received from a peer node to the appropriate
/// local subscriber(s). Handles unicast, group, and broadcast delivery, and
/// sends `ForwardAck` messages back when `require_ack` is set.
pub(crate) async fn deliver_forward_message(
    services: &Services,
    forward: ForwardMessage,
    local_node_id: &str,
) {
    // Save fields before moving.
    let forward_id = forward.forward_id;
    let original_source = forward.original_source;
    let require_ack = forward.require_ack;
    let title = forward.title;
    let payload = forward.payload;
    let is_broadcast = forward.is_broadcast;
    let is_group = forward.is_group;

    // Build RexData from the raw payload.
    let rex_data = RexData::unpack(BytesMut::from(payload.as_slice()));

    // --- Broadcast / Group: deliver to every local subscriber ---------
    if is_broadcast || is_group {
        let clients = services.registry.find_all_by_title(&title, None);
        if clients.is_empty() {
            warn!(
                "No local subscribers found for broadcast/group title: {}",
                title
            );
        } else {
            info!(
                "Delivering {} message to {} local subscribers",
                if is_broadcast { "broadcast" } else { "group" },
                clients.len()
            );
            for client in clients {
                let client_id = client.id();
                if let Err(e) = client.send_buf(rex_data.pack_ref()).await {
                    warn!("Failed to send to client {:032x}: {}", client_id, e);
                }
            }
        }
        return;
    }

    // --- Unicast ---------------------------------------------------
    let target_client = if forward.target_client_id != 0 {
        services.registry.find_some_by_id(forward.target_client_id)
    } else {
        services.registry.find_one_by_title(&title, None)
    };

    match target_client {
        Some(client) => {
            let client_id = client.id();
            info!(
                "Delivering forwarded message to local client {:032x}",
                client_id
            );

            if let Err(e) = client.send_buf(rex_data.pack_ref()).await {
                warn!("Failed to send to client {:032x}: {}", client_id, e);
                if require_ack {
                    let ack = ForwardAckMessage {
                        forward_id,
                        from_node_id: local_node_id.to_string(),
                        original_source,
                        success: false,
                        error: Some(e.to_string()),
                    };
                    services
                        .cluster
                        .broadcast(ClusterMessage::ForwardAck(ack))
                        .await;
                }
            } else {
                debug!(
                    "Successfully delivered forwarded message to {:032x}",
                    client_id
                );
                if require_ack {
                    let ack = ForwardAckMessage {
                        forward_id,
                        from_node_id: local_node_id.to_string(),
                        original_source,
                        success: true,
                        error: None,
                    };
                    services
                        .cluster
                        .broadcast(ClusterMessage::ForwardAck(ack))
                        .await;
                }
            }
        }
        None => {
            warn!("No local subscriber found for title: {}", title);
            if require_ack {
                let ack = ForwardAckMessage {
                    forward_id,
                    from_node_id: local_node_id.to_string(),
                    original_source,
                    success: false,
                    error: Some("No local subscriber".to_string()),
                };
                services
                    .cluster
                    .broadcast(ClusterMessage::ForwardAck(ack))
                    .await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{dummy_client_with_id, make_services};
    use rex_cluster::types::ForwardMessage;
    use rex_core::RexCommand;

    #[tokio::test]
    async fn deliver_unicast_to_existing_client() {
        let services = make_services(false);
        let local = "test-node";

        // Register a target client.
        let target = dummy_client_with_id(0x42u128);
        services.registry.add_client(target.clone());
        services.registry.register_title(0x42u128, "forwarded_chan");

        // Build a RexData payload.
        let rex_data = RexData::new(RexCommand::Title, "forwarded_chan", b"hello");
        let forward = ForwardMessage {
            forward_id: 1,
            original_source: 0xAAu128,
            target_client_id: 0,
            title: "forwarded_chan".to_string(),
            payload: rex_data.pack_ref().to_vec(),
            is_group: false,
            is_broadcast: false,
            require_ack: false,
        };

        deliver_forward_message(&services, forward, local).await;
    }

    #[tokio::test]
    async fn deliver_unicast_no_local_subscriber_is_silent() {
        let services = make_services(false);
        let local = "test-node";

        let rex_data = RexData::new(RexCommand::Title, "nobody", b"hello");
        let forward = ForwardMessage {
            forward_id: 2,
            original_source: 0xBBu128,
            target_client_id: 0,
            title: "nobody".to_string(),
            payload: rex_data.pack_ref().to_vec(),
            is_group: false,
            is_broadcast: false,
            require_ack: false,
        };

        // Should not panic, just log a warning.
        deliver_forward_message(&services, forward, local).await;
    }
}
