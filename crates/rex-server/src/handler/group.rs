use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rex_client::RexClientInner;
use rex_core::{RetCode, RexCommand, RexData};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data: &mut RexData,
) -> Result<()> {
    let title = data.title().unwrap_or_default();
    debug!("Received group message: {}", title);
    let client_id = data.header().source();

    let matching_clients = system.find_all_by_title(title, Some(client_id));

    if matching_clients.is_empty() {
        warn!("No clients found for group title: {}", title);
        if let Err(e) = source_client
            .send_buf(
                &data
                    .set_command(RexCommand::GroupReturn)
                    .set_retcode(RetCode::NoTargetAvailable)
                    .serialize(),
            )
            .await
        {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }
        return Ok(());
    }

    // 安全的轮询选择
    static GROUP_ROUND_ROBIN_INDEX: AtomicUsize = AtomicUsize::new(0);
    let index = GROUP_ROUND_ROBIN_INDEX.fetch_add(1, Ordering::Relaxed) % matching_clients.len();
    let target_client = &matching_clients[index];

    let target_client_id = target_client.id().await;
    data.set_target(target_client_id);

    if let Err(e) = target_client.send_buf(&data.serialize()).await {
        warn!("client [{:032X}] error: {}", target_client_id, e);
        if let Err(e) = source_client
            .send_buf(
                &data
                    .set_command(RexCommand::TitleReturn)
                    .set_retcode(RetCode::NoTargetAvailable)
                    .serialize(),
            )
            .await
        {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }
    }
    Ok(())
}
