use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexDataRef};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>,
) -> Result<()> {
    let client_id = data_ref.source();
    let title = data_ref.data_as_string_lossy();

    debug!("[{:032X}] Received reg title [{}]", client_id, title);

    if let Some(client) = system.find_some_by_id(client_id) {
        system.register_title(client_id, &title);

        let response = data_ref.create_response(RexCommand::RegTitleReturn, title.as_bytes());

        if let Err(e) = client.send_buf(&response.serialize()).await {
            warn!("[{:032X}] Send reg title return error: {}", client_id, e);
        }
    } else {
        let error_response = data_ref.create_error_response(
            RexCommand::RegTitleReturn,
            RetCode::NoTargetAvailable,
            b"Client not found",
        );

        let _ = source_client.send_buf(&error_response.serialize()).await;
    }

    Ok(())
}
