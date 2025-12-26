use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexDataRef};
use tracing::{debug, info, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>,
) -> Result<()> {
    let client_id = data_ref.source();
    debug!("[{:032X}] Received login message", client_id);

    // 使用零拷贝方法读取 title（避免分配）
    let title = data_ref.data_as_string_lossy();

    if let Some(client) = system.find_some_by_id(client_id) {
        warn!("[{:032X}] Client already exists", client_id);
        client.set_sender(source_client.sender());
        client.insert_title(&title);

        let response = data_ref.create_response(RexCommand::LoginReturn, &[]);

        if let Err(e) = client.send_buf(&response.serialize()).await {
            warn!(
                "[{:032X}] Send login return message error: {}",
                client_id, e
            );
        } else {
            info!(
                "Client [{:032X}] logged in with title: {}",
                client_id, title
            );
        }
    } else {
        source_client.set_id(client_id);
        source_client.insert_title(&title);
        system.add_client(source_client.clone()).await;

        let response = data_ref.create_response(RexCommand::LoginReturn, &[]);

        if let Err(e) = source_client.send_buf(&response.serialize()).await {
            warn!(
                "[{:032X}] Send login return message error: {}",
                client_id, e
            );
        } else {
            info!(
                "New client [{:032X}] logged in with title: {}",
                source_client.id(),
                title
            );
        }
    }

    Ok(())
}
