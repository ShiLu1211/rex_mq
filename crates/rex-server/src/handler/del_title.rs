use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_bytes: &mut [u8],
) -> Result<()> {
    let data = RexData::as_archive(data_bytes);
    let client_id: u128 = data.header.source.into();
    let title = data.title.as_str();
    debug!("[{:032X}] Received del title [{}]", client_id, title);

    if let Some(client) = system.find_some_by_id(client_id) {
        system.unregister_title(client_id, title);
        RexData::update_header(data_bytes, Some(RexCommand::DelTitleReturn), None, None);
        if let Err(e) = client.send_buf(data_bytes).await {
            warn!("[{:032X}] Send del title return error: {}", client_id, e);
        }
    } else {
        RexData::update_header(
            data_bytes,
            Some(RexCommand::DelTitleReturn),
            None,
            Some(RetCode::NoTarget),
        );
        if let Err(e) = source_client.send_buf(data_bytes).await {
            warn!("[{:032X}] Send del title return error: {}", client_id, e);
        }
    }
    Ok(())
}
