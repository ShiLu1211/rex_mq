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
    debug!("Received title message: {}", title);
    let client_id = data.header().source();

    let mut success = false;

    if let Some(target_client) = system.find_one_by_title(title, Some(client_id)) {
        data.set_target(target_client.id());
        if let Err(e) = target_client.send_buf(&data.serialize()).await {
            warn!(
                "client [{}] send to [{}] error: {}",
                client_id,
                target_client.id(),
                e
            );
        } else {
            success = true;
        }
    }

    if !success
        && let Err(e) = source_client
            .send_buf(
                &data
                    .set_command(RexCommand::TitleReturn)
                    .set_retcode(RetCode::NoTargetAvailable)
                    .serialize(),
            )
            .await
    {
        warn!("client [{}] error back: {}", client_id, e);
    }
    Ok(())
}
