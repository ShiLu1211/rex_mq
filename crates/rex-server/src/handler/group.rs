use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use std::time::Instant;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use rex_observability::metrics::{
    inc_messages_delivered, inc_messages_published, observe_publish_latency,
};
use scopeguard::guard;
use tracing::{debug, warn};

use crate::Services;
use crate::handler::port::CommandHandler;

pub struct GroupHandler;

impl CommandHandler for GroupHandler {
    async fn handle(
        &self,
        services: &Services,
        source_client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        let title = rex_data.title();
        debug!("Received group message: {}", title);
        let client_id: u128 = rex_data.source();

        // --- Observability: count + time every accepted publish ---
        // Group picks exactly one subscriber via round-robin, so we
        // record one publish and (on successful enqueue) one delivery.
        let started = Instant::now();
        let title_for_metric = title.to_string();
        inc_messages_published(&title_for_metric);
        let _metric_guard = guard((), |_| {
            observe_publish_latency(&title_for_metric, started.elapsed().as_secs_f64());
        });

        let matching_clients = services.registry.find_all_by_title(title, Some(client_id));

        if matching_clients.is_empty() {
            warn!("No clients found for group title: {}", title);
            if let Err(e) = source_client
                .send_buf(
                    rex_data
                        .set_command(RexCommand::GroupReturn)
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
        services.setup_message_ack(rex_data, client_id, title.to_string(), true);

        // 安全的轮询选择
        static GROUP_ROUND_ROBIN_INDEX: AtomicUsize = AtomicUsize::new(0);
        let index =
            GROUP_ROUND_ROBIN_INDEX.fetch_add(1, Ordering::Relaxed) % matching_clients.len();
        let target_client = &matching_clients[index];

        let target_client_id = target_client.id();

        if let Err(e) = target_client.send_buf(rex_data.pack_ref()).await {
            warn!("client [{:032X}] error: {}", target_client_id, e);
            if !services.is_ack_enabled()
                && let Err(e) = source_client
                    .send_buf(
                        rex_data
                            .set_command(RexCommand::GroupReturn)
                            .set_retcode(RetCode::NoTarget)
                            .pack_ref(),
                    )
                    .await
            {
                warn!("client [{:032X}] error back: {}", client_id, e);
            }
        } else {
            inc_messages_delivered(&title_for_metric, "local");
        }
        Ok(())
    }
}
