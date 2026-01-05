use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    _system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_bytes: &mut [u8],
) -> Result<()> {
    let data = RexData::as_archive(data_bytes);
    let client_id: u128 = data.header.source.into();
    debug!("[{:032X}] Received check online", client_id);
    RexData::update_header(data_bytes, Some(RexCommand::CheckReturn), None, None);
    if let Err(e) = source_client.send_buf(data_bytes).await {
        warn!(
            "[{:032X}] Send check return message error: {}",
            client_id, e
        );
    }
    Ok(())
}
