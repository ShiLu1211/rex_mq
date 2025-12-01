use std::sync::Arc;

use anyhow::Result;
use rex_client::RexClientInner;
use rex_core::{RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    _system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data: &mut RexData,
) -> Result<()> {
    let client_id = data.header().source();
    debug!("[{:032X}] Received check online", client_id);
    if let Err(e) = source_client
        .send_buf(&data.set_command(RexCommand::CheckReturn).serialize())
        .await
    {
        warn!(
            "[{:032X}] Send check return message error: {}",
            client_id, e
        );
    }
    Ok(())
}
