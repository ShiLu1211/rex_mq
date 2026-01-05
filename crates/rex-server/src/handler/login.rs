use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_bytes: &mut [u8],
) -> Result<()> {
    let data = RexData::as_archive(data_bytes);
    let client_id: u128 = data.header.source.into();
    debug!("[{:032X}] Received login message", client_id);
    let title = data.title.as_str().to_string();

    RexData::update_header(data_bytes, Some(RexCommand::LoginReturn), None, None);

    if let Some(client) = system.find_some_by_id(client_id) {
        warn!("[{:032X}] Client already exists", client_id);
        client.set_sender(source_client.sender().clone());
        client.insert_title(&title);

        if let Err(e) = client.send_buf(data_bytes).await {
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

        if let Err(e) = source_client.send_buf(data_bytes).await {
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
