use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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
    let title = data.title.as_str();
    debug!("Received group message: {}", title);
    let client_id: u128 = data.header.source.into();

    let matching_clients = system.find_all_by_title(title, Some(client_id));

    if matching_clients.is_empty() {
        warn!("No clients found for group title: {}", title);
        RexData::update_header(
            data_bytes,
            Some(RexCommand::GroupReturn),
            None,
            Some(RetCode::NoTarget),
        );
        if let Err(e) = source_client.send_buf(data_bytes).await {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }
        return Ok(());
    }

    // 安全的轮询选择
    static GROUP_ROUND_ROBIN_INDEX: AtomicUsize = AtomicUsize::new(0);
    let index = GROUP_ROUND_ROBIN_INDEX.fetch_add(1, Ordering::Relaxed) % matching_clients.len();
    let target_client = &matching_clients[index];

    let target_client_id = target_client.id();

    if let Err(e) = target_client.send_buf(data_bytes).await {
        warn!("client [{:032X}] error: {}", target_client_id, e);
        RexData::update_header(
            data_bytes,
            Some(RexCommand::GroupReturn),
            None,
            Some(RetCode::NoTarget),
        );
        if let Err(e) = source_client.send_buf(data_bytes).await {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }
    }
    Ok(())
}
