use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_bytes: &mut [u8],
) -> Result<()> {
    let data = RexData::as_archive(data_bytes);
    let title = data.title.as_str();
    let data_len = data.data.len();
    debug!("Received title message: {}", title);
    let client_id: u128 = data.header.source.into();

    let mut success = false;

    if let Some(target_client) = system.find_one_by_title(title, Some(client_id)) {
        let target_client_id = target_client.id();

        debug!(
            "client [{:032X}] title to [{:032X}] data_len[{}]",
            client_id, target_client_id, data_len
        );

        if let Err(e) = target_client.send_buf(data_bytes).await {
            warn!(
                "client [{:032X}] send to [{:032X}] error: {}",
                client_id, target_client_id, e
            );
        } else {
            success = true;
        }
    } else {
        info!("no target available for title [{}]", title);
    }

    if !success {
        RexData::update_header(
            data_bytes,
            Some(RexCommand::TitleReturn),
            None,
            Some(RetCode::NoTarget),
        );
        if let Err(e) = source_client.send_buf(data_bytes).await {
            warn!("client [{:032X}] error back: {}", client_id, e);
        } else {
            info!("client [{:032X}] title return", client_id);
        }
    }

    Ok(())
}
