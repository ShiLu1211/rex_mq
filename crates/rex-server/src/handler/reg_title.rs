use std::sync::Arc;

use anyhow::Result;
use rex_cluster::types::{ClusterMessage, TitleRegisterMessage};
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let client_id: u128 = rex_data.source();
    let title = rex_data.title().to_string();
    debug!("[{:032X}] Received reg title [{}]", client_id, title);

    if let Some(client) = system.find_some_by_id(client_id) {
        system.register_title(client_id, &title);

        // Broadcast title registration to cluster
        if let Some(cluster) = system.cluster_manager() {
            let local_node_id = cluster.local_node_id().to_string();
            let register_msg = TitleRegisterMessage {
                node_id: local_node_id,
                title: title.clone(),
                client_id,
            };
            let cluster_msg = ClusterMessage::TitleRegister(register_msg);
            cluster.broadcast(cluster_msg).await;
            debug!("Broadcasted title registration for [{}] to cluster", title);
        }

        if let Err(e) = client
            .send_buf(rex_data.set_command(RexCommand::RegTitleReturn).pack_ref())
            .await
        {
            warn!("[{:032X}] Send reg title return error: {}", client_id, e);
        }
    } else if let Err(e) = source_client
        .send_buf(
            rex_data
                .set_command(RexCommand::RegTitleReturn)
                .set_retcode(RetCode::NoTarget)
                .pack_ref(),
        )
        .await
    {
        warn!("[{:032X}] Send reg title return error: {}", client_id, e);
    }
    Ok(())
}
