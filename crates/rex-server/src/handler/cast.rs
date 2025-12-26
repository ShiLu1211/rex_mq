use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData, RexDataRef};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>,
) -> Result<()> {
    let title = data_ref.title().unwrap_or_default();
    let client_id = data_ref.source();

    debug!("Received cast message: {}", title);

    let matching_clients = system.find_all_by_title(title, Some(client_id));

    if matching_clients.is_empty() {
        warn!("No clients found for cast title: {}", title);

        let error_response = data_ref.create_error_response(
            RexCommand::CastReturn,
            RetCode::NoTargetAvailable,
            b"No targets available",
        );

        let _ = source_client.send_buf(&error_response.serialize()).await;
        return Ok(());
    }

    let mut success_count = 0;
    let mut failed_clients = Vec::new();

    // 只在第一次需要时序列化消息
    let serialized_once = {
        let mut msg = RexData::new(
            data_ref.command(),
            data_ref.source(),
            0, // 稍后设置 target
            bytes::BytesMut::from(data_ref.data()),
        );
        if let Some(t) = data_ref.title() {
            msg.set_title(Some(t.to_string()));
        }
        msg
    };

    for client in matching_clients {
        let client_id = client.id();

        // 为每个客户端设置不同的 target
        let mut msg = serialized_once.clone();
        msg.set_target(client_id);
        let serialized = msg.serialize();

        if let Err(e) = client.send_buf(&serialized).await {
            warn!(
                "Failed to send cast message to client [{:032X}]: {}",
                client_id, e
            );
            failed_clients.push(client_id);
        } else {
            success_count += 1;
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

    if success_count == 0 {
        let error_response = data_ref.create_error_response(
            RexCommand::CastReturn,
            RetCode::NoTargetAvailable,
            b"All targets unreachable",
        );
        let _ = source_client.send_buf(&error_response.serialize()).await;
    }

    Ok(())
}
