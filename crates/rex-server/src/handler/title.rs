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
    let title = rex_data.title();
    let data_len = rex_data.data().len();
    debug!("Received title message: {}", title);
    let client_id: u128 = rex_data.source();

    let mut success = false;

    if let Some(target_client) = system.find_one_by_title(title, Some(client_id)) {
        let target_client_id = target_client.id();

        debug!(
            "client [{:032X}] title to [{:032X}] data_len[{}]",
            client_id, target_client_id, data_len
        );

        // Generate message ID for ACK if enabled
        if system.is_ack_enabled() {
            let title_clone = title.to_string();
            // Use the message_id from client if already set, otherwise generate a new one
            let msg_id = if rex_data.message_id() != 0 {
                rex_data.message_id()
            } else {
                fastrand::u64(..)
            };
            rex_data.set_message_id(msg_id);

            // Register pending ACK
            system.register_pending_ack(
                msg_id,
                client_id,
                title_clone,
                false, // Title is unicast, not group
            );
        }

        if let Err(e) = target_client.send_buf(rex_data.pack_ref()).await {
            warn!(
                "client [{:032X}] send to [{:032X}] error: {}",
                client_id, target_client_id, e
            );
        } else {
            success = true;
        }
    } else {
        info!("no target available for title [{}]", title);
    }

    // Send error back if no success
    // Note: When ACK is enabled and message is sent successfully, we wait for ACK
    // so we don't send TitleReturn immediately. But if there's no target, we should
    // still return an error.
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
            info!("client [{:032X}] title return", client_id);
        }
    }

    Ok(())
}
