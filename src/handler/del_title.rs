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
    let client_id = data.header().source();
    let title = data.data_as_string_lossy();
    debug!("[{}] Received del title [{}]", client_id, title);

    if let Some(client) = system.find_some_by_id(client_id) {
        system.unregister_title(client_id, &title);
        if let Err(e) = client
            .send_buf(&data.set_command(RexCommand::DelTitleReturn).serialize())
            .await
        {
            warn!("[{}] Send del title return error: {}", client_id, e);
        }
    } else if let Err(e) = source_client
        .send_buf(
            &data
                .set_command(RexCommand::DelTitleReturn)
                .set_retcode(RetCode::NoTargetAvailable)
                .serialize(),
        )
        .await
    {
        warn!("[{}] Send del title return error: {}", client_id, e);
    }
    Ok(())
}
