use std::sync::Arc;

use anyhow::Result;
use futures::{StreamExt, stream::FuturesUnordered};
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use rex_observability::metrics::{inc_messages_delivered, inc_messages_published};
use tracing::{debug, warn};

use crate::Services;
use crate::handler::port::CommandHandler;

pub struct CastHandler;

impl CommandHandler for CastHandler {
    async fn handle(
        &self,
        services: &Services,
        source_client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        let title = rex_data.title();
        debug!("Received cast message: {}", title);
        let client_id = rex_data.source();

        // Per-title publish counter: cast fans out to N subscribers, so
        // we count once per inbound command (here) and per-subscriber
        // delivery on success (below). The dispatch-level wrap handles
        // per-command + duration; this is per-title, which has different
        // cardinality.
        let title_for_metric = title.to_string();
        inc_messages_published(&title_for_metric);

        let matching_clients = services.registry.find_all_by_title(title, Some(client_id));

        if matching_clients.is_empty() {
            warn!("No clients found for cast title: {}", title);
            if let Err(e) = source_client
                .send_buf(
                    rex_data
                        .set_command(RexCommand::CastReturn)
                        .set_retcode(RetCode::NoTarget)
                        .pack_ref(),
                )
                .await
            {
                warn!("client [{:032X}] error back: {}", client_id, e);
            }
            return Ok(());
        }

        // ACK setup: generate msg_id and register pending ACK.
        services.setup_message_ack(rex_data, client_id, title.to_string(), false);

        // 并行发送 - 复用 buf 避免重复打包
        let buf = rex_data.pack_ref();
        let tasks: FuturesUnordered<_> = matching_clients
            .into_iter()
            .map(|client| async move {
                let client_id = client.id();
                match client.send_buf(buf).await {
                    Ok(()) => (client_id, true),
                    Err(e) => {
                        warn!("Failed to send to client [{:032X}]: {}", client_id, e);
                        (client_id, false)
                    }
                }
            })
            .collect();

        let mut failed_clients = Vec::new();
        let mut success_count = 0;

        // 并发收集结果
        for (client_id, success) in tasks.collect::<Vec<_>>().await {
            if success {
                success_count += 1;
            } else {
                failed_clients.push(client_id);
            }
        }

        debug!(
            "Cast message sent to {} clients, {} failures",
            success_count,
            failed_clients.len()
        );

        // At least one subscriber received the message — count it as
        // delivered. Failures are scrubbed via `remove_client` below.
        if success_count > 0 {
            inc_messages_delivered(&title_for_metric, "local");
        }

        // 清理发送失败的客户端
        for failed_client_id in failed_clients {
            services.remove_client(failed_client_id).await;
        }

        // If no clients received the message successfully, send error back
        if success_count == 0
            && let Err(e) = source_client
                .send_buf(
                    rex_data
                        .set_command(RexCommand::CastReturn)
                        .set_retcode(RetCode::NoTarget)
                        .pack_ref(),
                )
                .await
        {
            warn!("client [{:032X}] error back: {}", client_id, e);
        }

        // If ACK is enabled and at least one client received the message,
        // we wait for ACK from receivers. Don't send CastReturn yet.

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{dummy_client_with_id, make_services};

    #[tokio::test]
    async fn cast_no_subscribers_returns_no_target() {
        let services = make_services(false);
        let source = dummy_client_with_id(0x1u128);
        let mut rex_data = RexData::new(RexCommand::Cast, "absent", b"hello");
        rex_data.set_source(0x1u128);
        assert!(
            CastHandler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn cast_delivers_to_all_subscribers() {
        let services = make_services(false);
        let source_id = 0xAAAu128;
        let source = dummy_client_with_id(source_id);
        let sub1 = dummy_client_with_id(0xBBBu128);
        let sub2 = dummy_client_with_id(0xCCCu128);

        services.registry.add_client(sub1.clone());
        services.registry.register_title(sub1.id(), "cast_chan");
        services.registry.add_client(sub2.clone());
        services.registry.register_title(sub2.id(), "cast_chan");

        let mut rex_data = RexData::new(RexCommand::Cast, "cast_chan", b"hello");
        rex_data.set_source(source_id);
        assert!(
            CastHandler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
    }
}
