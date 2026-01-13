use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let client_id: u128 = rex_data.source();
    let title = rex_data.title();
    debug!("[{:032X}] Received del title [{}]", client_id, title);

    if let Some(client) = system.find_some_by_id(client_id) {
        system.unregister_title(client_id, title);
        if let Err(e) = client
            .send_buf(rex_data.set_command(RexCommand::DelTitleReturn).pack_ref())
            .await
        {
            warn!("[{:032X}] Send del title return error: {}", client_id, e);
        }
    } else if let Err(e) = source_client
        .send_buf(
            rex_data
                .set_command(RexCommand::DelTitleReturn)
                .set_retcode(RetCode::NoTarget)
                .pack_ref(),
        )
        .await
    {
        warn!("[{:032X}] Send del title return error: {}", client_id, e);
    }
    Ok(())
}
