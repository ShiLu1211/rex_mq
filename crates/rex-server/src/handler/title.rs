use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let title = rex_data.title().to_string();
    let data_len = rex_data.data().len();
    debug!("Received title message: {}", title);
    let client_id: u128 = rex_data.source();

    let mut success = false;

    // First, try to find local target
    if let Some(target_client) = system.find_one_by_title(&title, Some(client_id)) {
        let target_client_id = target_client.id();

        debug!(
            "client [{:032X}] title to local [{:032X}] data_len[{}]",
            client_id, target_client_id, data_len
        );

        success = deliver_message(system, source_client, rex_data, &target_client).await;
    } else {
        // No local target, try to find remote target via cluster
        info!("no local target for title [{}], checking cluster", title);

        let mut tried_nodes = Vec::new();

        // First try the specific target node from route table
        if let Some(target_node) = system.find_node_for_title(&title) {
            debug!(
                "route table returned node: {} for title: {}",
                target_node, title
            );

            if let Some(local_id) = system.get_local_node_id() {
                debug!(
                    "comparing target_node={} with local_id={}",
                    target_node, local_id
                );

                if target_node != local_id {
                    // Target is on another node, forward the message
                    debug!("forwarding title [{}] to node {}", title, target_node);
                    tried_nodes.push(target_node.clone());
                    success = forward_to_node(system, source_client, rex_data, &target_node).await;
                } else {
                    // Target is local node but no local subscriber
                    // This could happen if route table is stale or title registered on another node
                    info!("target node is local but no local subscriber found, trying other nodes");
                }
            }
        } else {
            debug!("route table returned None for title: {}", title);
        }

        // If not successful, try all other cluster nodes
        if !success && let Some(cluster) = system.cluster_manager() {
            let all_nodes = cluster.get_nodes();
            for node in all_nodes {
                if !tried_nodes.contains(&node) {
                    let local_id = system.get_local_node_id().unwrap_or_default();
                    if node != local_id {
                        debug!("trying to forward to node {} for title [{}]", node, title);
                        if forward_to_node(system, source_client, rex_data, &node).await {
                            success = true;
                            break;
                        }
                    }
                }
            }
        }

        if !success {
            info!("no target available for title [{}]", title);
        }
    }

    // Send error back if no success
    if !success {
        if let Err(e) = source_client
            .send_buf(
                rex_data
                    .set_command(RexCommand::TitleReturn)
                    .set_retcode(RetCode::NoTarget)
                    .pack_ref(),
            )
            .await
        {
            warn!("client [{:032X}] error back: {}", client_id, e);
        } else {
            debug!("client [{:032X}] title return no target", client_id);
        }
    }

    Ok(())
}

/// Deliver message to local target client
async fn deliver_message(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
    target_client: &Arc<RexClientInner>,
) -> bool {
    let client_id = source_client.id();
    let target_client_id = target_client.id();

    // Generate message ID for ACK if enabled
    if system.is_ack_enabled() {
        let title = rex_data.title().to_string();
        let msg_id = if rex_data.message_id() != 0 {
            rex_data.message_id()
        } else {
            fastrand::u64(..)
        };
        rex_data.set_message_id(msg_id);

        system.register_pending_ack(msg_id, client_id, title, false);
    }

    if let Err(e) = target_client.send_buf(rex_data.pack_ref()).await {
        warn!(
            "client [{:032X}] send to [{:032X}] error: {}",
            client_id, target_client_id, e
        );
        false
    } else {
        debug!("delivered to local client [{:032X}]", target_client_id);
        true
    }
}

/// Forward message to another node
async fn forward_to_node(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
    target_node: &str,
) -> bool {
    let client_id = source_client.id();

    // Forward via cluster
    if let Some(cluster) = system.cluster_manager() {
        let forward_request = crate::ForwardRequest {
            source_client_id: client_id,
            target_client_id: 0, // Let target node find the subscriber
            title: rex_data.title().to_string(),
            payload: rex_data.pack_ref().to_vec(),
            msg_type: crate::ForwardType::Unicast,
        };

        if cluster.forward_message(target_node, forward_request).await {
            debug!("forwarded to node {}", target_node);
            return true;
        } else {
            warn!("failed to forward to node {}", target_node);
        }
    }

    false
}
