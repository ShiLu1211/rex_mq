use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexDataRef};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    _system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>,
) -> Result<()> {
    let client_id = data_ref.source();
    debug!("[{:032X}] Received check online", client_id);

    // 零拷贝创建响应
    let response = data_ref.create_response(RexCommand::CheckReturn, &[]);

    if let Err(e) = source_client.send_buf(&response.serialize()).await {
        warn!(
            "[{:032X}] Send check return message error: {}",
            client_id, e
        );
    }

    Ok(())
}
