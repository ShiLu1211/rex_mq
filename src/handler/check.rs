use std::sync::Arc;

use anyhow::Result;
use tracing::{debug, warn};

use crate::{
    RexClientInner, RexSystem,
    protocol::{RexCommand, RexData},
};

pub async fn handle(
    _system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data: &mut RexData,
) -> Result<()> {
    let client_id = data.header().source();
    debug!("[{:032X}] Received check online", client_id);
    if let Err(e) = source_client
        .send_buf(&data.set_command(RexCommand::CastReturn).serialize())
        .await
    {
        warn!(
            "[{:032X}] Send check return message error: {}",
            client_id, e
        );
    }
    Ok(())
}
