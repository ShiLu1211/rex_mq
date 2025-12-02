use std::sync::Arc;

use anyhow::Result;
use rex_client::RexClientInner;
use rex_core::{RetCode, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data: &mut RexData,
) -> Result<()> {
    let title = data.title().unwrap_or_default();
    debug!("Received title message: {}", title);
    let client_id = data.header().source();

    let mut success = false;

    if let Some(target_client) = system.find_one_by_title(title, Some(client_id)) {
        let target_client_id = target_client.id();
        data.set_target(target_client_id);

        debug!(
            "client [{:032X}] title to [{:032X}] data_len[{}]",
            client_id,
            target_client_id,
            data.data().len()
        );

        if let Err(e) = target_client.send_buf(&data.serialize()).await {
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

    if !success {
        if let Err(e) = source_client
            .send_buf(
                &data
                    .set_command(RexCommand::TitleReturn)
                    .set_retcode(RetCode::NoTargetAvailable)
                    .serialize(),
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
