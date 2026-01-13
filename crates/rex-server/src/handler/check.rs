use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    _system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let client_id: u128 = rex_data.source();
    debug!("[{:032X}] Received check online", client_id);
    if let Err(e) = source_client
        .send_buf(rex_data.set_command(RexCommand::CheckReturn).pack_ref())
        .await
    {
        warn!(
            "[{:032X}] Send check return message error: {}",
            client_id, e
        );
    }
    Ok(())
}
