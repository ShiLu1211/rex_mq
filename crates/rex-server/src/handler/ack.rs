// src/handler/ack.rs

use std::sync::Arc;

use anyhow::Result;
use rex_core::{AckData, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

/// Handle ACK from receiver to sender
///
/// When a receiver gets a message and wants to acknowledge it,
/// the server forwards the ACK to the original sender.
pub async fn handle(
    system: &Arc<RexSystem>,
    _source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    if !system.is_ack_enabled() {
        debug!("ACK received but ACK is not enabled, ignoring");
        return Ok(());
    }

    // Parse the ACK data
    let ack_data = AckData::from_rex_data(rex_data);
    let message_id = ack_data.message_id;

    debug!("Received ACK for message: {}", message_id);

    // Look up the original sender
    if let Some(pending_ack) = system.take_pending_ack(message_id) {
        // Forward ACK to the original sender
        if let Some(sender) = system.find_some_by_id(pending_ack.source_client_id) {
            // Create a new AckData to forward
            let ack_to_send = AckData::new(message_id);
            let rex_data =
                ack_to_send.to_rex_data(pending_ack.source_client_id, RexCommand::AckReturn);

            if let Err(e) = sender.send_buf(rex_data.pack_ref()).await {
                warn!(
                    "Failed to forward ACK to sender [{:032X}]: {}",
                    pending_ack.source_client_id, e
                );
            } else {
                debug!(
                    "ACK forwarded to sender [{:032X}]",
                    pending_ack.source_client_id
                );
            }
        } else {
            warn!(
                "Sender [{:032X}] not found for ACK message {}",
                pending_ack.source_client_id, message_id
            );
        }
    } else {
        debug!("No pending ACK found for message: {}", message_id);
    }

    Ok(())
}
