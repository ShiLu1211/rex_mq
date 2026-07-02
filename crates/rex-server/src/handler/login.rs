use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::Services;

pub async fn handle(
    services: &Services,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let client_id: u128 = rex_data.source();
    debug!("[{:032X}] Received login message", client_id);
    let title = rex_data.title().to_owned();

    if let Some(client) = services.registry.find_some_by_id(client_id) {
        warn!("[{:032X}] Client already exists", client_id);
        client.set_sender(source_client.sender().clone());
        client.insert_title(&title);

        if let Err(e) = client
            .send_buf(rex_data.set_command(RexCommand::LoginReturn).pack_ref())
            .await
        {
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

        services.add_client(source_client.clone()).await;

        // Drain any messages queued for this client ID while it was offline.
        let queued = services.get_offline_messages(client_id).await;
        if !queued.is_empty() {
            info!(
                "Client [{:032X}] reconnecting with {} queued offline message(s)",
                client_id,
                queued.len()
            );
            for msg in queued {
                let mut title_data = RexData::new(RexCommand::Title, &msg.title, &msg.payload);
                title_data.set_source(client_id);
                title_data.set_message_id(msg.id);
                if let Err(e) = source_client.send_buf(title_data.pack_ref()).await {
                    warn!(
                        "Failed to deliver queued offline message [{:032X}] to client: {}",
                        msg.id, e
                    );
                }
            }
            services.clear_offline_messages(client_id).await;
        }

        if let Err(e) = source_client
            .send_buf(rex_data.set_command(RexCommand::LoginReturn).pack_ref())
            .await
        {
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
