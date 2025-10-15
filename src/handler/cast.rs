use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, warn};

use crate::{
    RexClientInner, RexSystem,
    protocol::{RetCode, RexCommand, RexData},
};

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data: &mut RexData,
) -> Result<()> {
    let title = data.title().unwrap_or_default();
    debug!("Received cast message: {}", title);
    let client_id = data.header().source();

    let matching_clients = system.find_all_by_title(title, Some(client_id));

    if matching_clients.is_empty() {
        warn!("No clients found for cast title: {}", title);
        if let Err(e) = source_client
            .send_buf(
                &data
                    .set_command(RexCommand::CastReturn)
                    .set_retcode(RetCode::NoTargetAvailable)
                    .serialize(),
            )
            .await
        {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }
        return Ok(());
    }

    let mut success_count = 0;
    let mut failed_clients = Vec::new();

    for client in matching_clients {
        data.set_target(client.id());

        if let Err(e) = client.send_buf(&data.serialize()).await {
            warn!(
                "Failed to send cast message to client [{:032X}]: {}",
                client.id(),
                e
            );
            failed_clients.push(client.id());
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

    if success_count == 0
        && let Err(e) = source_client
            .send_buf(
                &data
                    .set_command(RexCommand::CastReturn)
                    .set_retcode(RetCode::NoTargetAvailable)
                    .serialize(),
            )
            .await
    {
        warn!("client [{:032X}] error back: {}", client_id, e);
    }
    Ok(())
}
