use std::sync::Arc;

use anyhow::Result;
use futures::{StreamExt, stream::FuturesUnordered};
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let title = rex_data.title();
    debug!("Received cast message: {}", title);
    let client_id = rex_data.source();

    let matching_clients = system.find_all_by_title(title, Some(client_id));

    if matching_clients.is_empty() {
        warn!("No clients found for cast title: {}", title);
        if let Err(e) = source_client
            .send_buf(
                rex_data
                    .set_command(RexCommand::CastReturn)
                    .set_retcode(RetCode::NoTarget)
                    .pack_ref(),
            )
            .await
        {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }
        return Ok(());
    }

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
            false, // Cast is not group
        );
    }

    // 并行发送 - 复用 buf 避免重复打包
    let buf = rex_data.pack_ref();
    let tasks: FuturesUnordered<_> = matching_clients
        .into_iter()
        .map(|client| async move {
            let client_id = client.id();
            match client.send_buf(buf).await {
                Ok(()) => (client_id, true),
                Err(e) => {
                    warn!("Failed to send to client [{:032X}]: {}", client_id, e);
                    (client_id, false)
                }
            }
        })
        .collect();

    let mut failed_clients = Vec::new();
    let mut success_count = 0;

    // 并发收集结果
    for (client_id, success) in tasks.collect::<Vec<_>>().await {
        if success {
            success_count += 1;
        } else {
            failed_clients.push(client_id);
        }
    }

    debug!(
        "Cast message sent to {} clients, {} failures",
        success_count,
        failed_clients.len()
    );

    // 清理发送失败的客户端
    for failed_client_id in failed_clients {
        system.remove_client(failed_client_id).await;
    }

    // If no clients received the message successfully, send error back
    if success_count == 0
        && let Err(e) = source_client
            .send_buf(
                rex_data
                    .set_command(RexCommand::CastReturn)
                    .set_retcode(RetCode::NoTarget)
                    .pack_ref(),
            )
            .await
    {
        warn!("client [{:032X}] error back: {}", client_id, e);
    }

    // If ACK is enabled and at least one client received the message,
    // we wait for ACK from receivers. Don't send CastReturn yet.

    Ok(())
}
