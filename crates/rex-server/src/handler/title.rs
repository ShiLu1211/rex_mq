use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use rex_observability::metrics::{
    inc_forward_failures, inc_messages_delivered, inc_messages_published, observe_deliver_latency,
};
use tracing::{debug, info, warn};

use crate::handler::port::CommandHandler;
use crate::{FwdResult, RoutePlan, Services};

pub struct TitleHandler;

impl CommandHandler for TitleHandler {
    async fn handle(
        &self,
        services: &Services,
        source_client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        let title = rex_data.title().to_string();
        let data_len = rex_data.data().len();
        debug!("Received title message: {}", title);
        let client_id: u128 = rex_data.source();

        // Per-title publish counter (the dispatch-level wrap handles
        // per-command + duration). `local_deliver_started` snapshots the
        // time we accepted the publish, used by `observe_deliver_latency`
        // on the local-subscriber path below.
        let title_for_metric = title.clone();
        let local_deliver_started = Instant::now();
        inc_messages_published(&title_for_metric);

        let mut success = false;

        match services.router.route(&title, Some(client_id)) {
            RoutePlan::Local(target) => {
                let target_client_id = target.id();
                debug!(
                    "client [{:032X}] title to local [{:032X}] data_len[{}]",
                    client_id, target_client_id, data_len
                );
                success = deliver_message(services, source_client, rex_data, &target).await;
                if success {
                    inc_messages_delivered(&title_for_metric, "local");
                    // Observability: publish→subscriber enqueue latency
                    // (local case only — remote is bounded by the
                    // forward call, not by our local enqueue).
                    observe_deliver_latency(
                        &title_for_metric,
                        local_deliver_started.elapsed().as_secs_f64(),
                    );
                }
            }
            RoutePlan::Remote(node) => {
                info!(
                    "no local target for title [{}], forwarding to node {}",
                    title, node
                );

                let request = crate::ForwardRequest {
                    source_client_id: client_id,
                    target_client_id: 0,
                    title: title.clone(),
                    payload: rex_data.pack_ref().to_vec(),
                    msg_type: crate::ForwardType::Unicast,
                };

                // Why this works: Forwarder::forward already does the
                // direct send + fallback walk over other known peers
                // (forwarder.rs:222-256) and returns the structured
                // FwdResult. ADR-0003 records that this is the only
                // entry point for cross-node wire I/O; the handler
                // just maps the outcome to metrics + the no-target
                // path below. This replaces the previous in-handler
                // `for other in cluster.get_nodes()` loop, which
                // duplicated that fallback logic and conflated every
                // failure mode under a single "unreachable" label.
                let outcome = services.forwarder.forward(&node, &request).await;
                match outcome {
                    FwdResult::Delivered => {
                        success = true;
                    }
                    FwdResult::PeerUnreachable(reason) => {
                        inc_forward_failures("unreachable");
                        warn!("title '{}' forward to {} failed: {}", title, node, reason);
                    }
                    FwdResult::NoPeerForTitle => {
                        // New in this rewrite: previously the inline
                        // fallback walked peers but silently dropped
                        // the message when no peers were known. This
                        // surfaces that condition as its own metric
                        // reason so ops can distinguish "no peers"
                        // from "peer unreachable".
                        inc_forward_failures("no_peer");
                    }
                }

                if success {
                    inc_messages_delivered(&title_for_metric, "remote");
                }
            }
            RoutePlan::None => {
                info!("no target available for title [{}]", title);
            }
        }

        if !success {
            if let Err(e) = source_client
                .send_buf(
                    rex_data
                        .set_command(RexCommand::TitleReturn)
                        .set_retcode(RetCode::NoTarget)
                        .pack_ref(),
                )
                .await
            {
                warn!("client [{:032X}] error back: {}", client_id, e);
            } else {
                debug!("client [{:032X}] title return no target", client_id);
            }
        }

        Ok(())
    }
}

/// Deliver message to local target client
async fn deliver_message(
    services: &Services,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
    target_client: &Arc<RexClientInner>,
) -> bool {
    let client_id = source_client.id();
    let target_client_id = target_client.id();

    services.setup_message_ack(rex_data, client_id, rex_data.title().to_string(), false);

    if let Err(e) = target_client.send_buf(rex_data.pack_ref()).await {
        warn!(
            "client [{:032X}] send to [{:032X}] error: {}",
            client_id, target_client_id, e
        );
        false
    } else {
        debug!("delivered to local client [{:032X}]", target_client_id);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{dummy_client_with_id, make_services};

    #[tokio::test]
    async fn title_no_subscriber_returns_no_target() {
        let services = make_services(false);
        let source = dummy_client_with_id(0x1u128);
        let mut rex_data = RexData::new(RexCommand::Title, "no_subscribers", b"hello");
        rex_data.set_source(0x1u128);
        // No subscribers, no cluster node — should not panic, returns Ok.
        assert!(
            TitleHandler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
    }

    #[tokio::test]
    async fn title_delivers_to_local_subscriber() {
        let services = make_services(false);
        let source_id = 0xABCu128;
        let target_id = 0xDEFu128;
        let source = dummy_client_with_id(source_id);
        let target = dummy_client_with_id(target_id);

        // Register target in the registry
        services.registry.add_client(target.clone());
        services.registry.register_title(target_id, "local_only");

        let mut rex_data = RexData::new(RexCommand::Title, "local_only", b"hello");
        rex_data.set_source(source_id);
        assert!(
            TitleHandler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
    }
}
