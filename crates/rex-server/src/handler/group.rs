use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData, RexDataRef};
use tracing::{debug, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>,
) -> Result<()> {
    let title = data_ref.title().unwrap_or_default();
    let client_id = data_ref.source();

    debug!("Received group message: {}", title);

    let matching_clients = system.find_all_by_title(title, Some(client_id));

    if matching_clients.is_empty() {
        warn!("No clients found for group title: {}", title);

        let error_response = data_ref.create_error_response(
            RexCommand::GroupReturn,
            RetCode::NoTargetAvailable,
            b"No targets available",
        );

        let _ = source_client.send_buf(&error_response.serialize()).await;
        return Ok(());
    }

    // 轮询选择
    static GROUP_ROUND_ROBIN_INDEX: AtomicUsize = AtomicUsize::new(0);
    let index = GROUP_ROUND_ROBIN_INDEX.fetch_add(1, Ordering::Relaxed) % matching_clients.len();
    let target_client = &matching_clients[index];
    let target_client_id = target_client.id();

    // 零拷贝创建消息
    let mut msg = RexData::new(
        data_ref.command(),
        data_ref.source(),
        target_client_id,
        data_ref.data().into(),
    );
    if let Some(t) = data_ref.title() {
        msg.set_title(Some(t.to_string()));
    }

    if let Err(e) = target_client.send_buf(&msg.serialize()).await {
        warn!("client [{:032X}] error: {}", target_client_id, e);

        let error_response = data_ref.create_error_response(
            RexCommand::GroupReturn,
            RetCode::NoTargetAvailable,
            b"Target unreachable",
        );

        let _ = source_client.send_buf(&error_response.serialize()).await;
    }

    Ok(())
}
