use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData, RexDataRef};
use tracing::{debug, info, warn};

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    source_client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>,
) -> Result<()> {
    let title = data_ref.title().unwrap_or_default();
    let client_id = data_ref.source();

    debug!("Received title message: {}", title);

    if let Some(target_client) = system.find_one_by_title(title, Some(client_id)) {
        let target_client_id = target_client.id();

        debug!(
            "client [{:032X}] title to [{:032X}] data_len[{}]",
            client_id,
            target_client_id,
            data_ref.data_len()
        );

        // 零拷贝创建响应 - 只在需要时反序列化
        let mut response = RexData::new(
            data_ref.command(),
            data_ref.source(),
            target_client_id,
            data_ref.data().into(),
        );
        response.set_target(target_client_id);
        if let Some(t) = data_ref.title() {
            response.set_title(Some(t.to_string()));
        }

        if let Err(e) = target_client.send_buf(&response.serialize()).await {
            warn!(
                "client [{:032X}] send to [{:032X}] error: {}",
                client_id, target_client_id, e
            );

            // 发送错误响应
            let error_response = data_ref.create_error_response(
                RexCommand::TitleReturn,
                RetCode::NoTargetAvailable,
                b"Target unreachable",
            );
            let _ = source_client.send_buf(&error_response.serialize()).await;
        }
    } else {
        info!("no target available for title [{}]", title);

        // 使用零拷贝创建错误响应
        let error_response = data_ref.create_error_response(
            RexCommand::TitleReturn,
            RetCode::NoTargetAvailable,
            b"No target available",
        );

        if let Err(e) = source_client.send_buf(&error_response.serialize()).await {
            warn!("client [{:032X}] error back: {}", client_id, e);
        } else {
            info!("client [{:032X}] title return", client_id);
        }
    }

    Ok(())
}
