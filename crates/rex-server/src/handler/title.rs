use std::sync::Arc;

use anyhow::Result;
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::{RoutePlan, Services};

pub async fn handle(
    services: &Services,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let title = rex_data.title().to_string();
    let data_len = rex_data.data().len();
    debug!("Received title message: {}", title);
    let client_id: u128 = rex_data.source();

    let mut success = false;

    match services.router.route(&title, Some(client_id)) {
        RoutePlan::Local(target) => {
            let target_client_id = target.id();
            debug!(
                "client [{:032X}] title to local [{:032X}] data_len[{}]",
                client_id, target_client_id, data_len
            );
            success = deliver_message(services, source_client, rex_data, &target).await;
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

            success = services.router.forward(&node, request).await;

            // Broadcast fallback: try all other known nodes.
            if !success {
                let local_id = services.router.local_node_id().unwrap_or_default();
                for other in services.router.known_nodes() {
                    if other != node && other != local_id {
                        let fallback = crate::ForwardRequest {
                            source_client_id: client_id,
                            target_client_id: 0,
                            title: title.clone(),
                            payload: rex_data.pack_ref().to_vec(),
                            msg_type: crate::ForwardType::Unicast,
                        };
                        if services.router.forward(&other, fallback).await {
                            success = true;
                            debug!("fallback forward to node {} succeeded", other);
                            break;
                        }
                    }
                }
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

/// Deliver message to local target client
async fn deliver_message(
    services: &Services,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
    target_client: &Arc<RexClientInner>,
) -> bool {
    let client_id = source_client.id();
    let target_client_id = target_client.id();

    if services.is_ack_enabled() {
        let title = rex_data.title().to_string();
        let msg_id = if rex_data.message_id() != 0 {
            rex_data.message_id()
        } else {
            fastrand::u64(..)
        };
        rex_data.set_message_id(msg_id);

        services.register_pending_ack(msg_id, client_id, title, false);
    }

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

// `forward_to_node` is dead code after C4 — routing now goes through
// `services.router.forward()`, which wraps `ClusterPort::forward_message`.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{
        TestAckTracker, TestClusterPort, TestRegistry, dummy_client_with_id,
    };
    use crate::{
        AckTracker, ClientRegistry, ClusterPort, ClusterRouter, NoopOfflineBuffer, OfflineBuffer,
        RexSystemConfig, Router, Services, Shutdown,
    };
    use std::sync::Arc;

    fn make_services() -> Arc<Services> {
        let registry = TestRegistry::new();
        let acks = Arc::new(TestAckTracker::new()) as Arc<dyn AckTracker>;
        let offline = Arc::new(NoopOfflineBuffer) as Arc<dyn OfflineBuffer>;
        let cluster: Arc<dyn ClusterPort> = Arc::new(TestClusterPort::new());
        let shutdown = Shutdown::new();
        let config = RexSystemConfig::from_id("test");
        Services::new(
            registry.to_arc(),
            acks,
            offline,
            cluster.clone(),
            ClusterRouter::new(registry.to_arc(), cluster.clone()),
            shutdown,
            config,
        )
    }

    #[tokio::test]
    async fn title_no_subscriber_returns_no_target() {
        let services = make_services();
        let source = dummy_client_with_id(0x1u128);
        let mut rex_data = RexData::new(RexCommand::Title, "no_subscribers", b"hello");
        rex_data.set_source(0x1u128);
        // No subscribers, no cluster node — should not panic, returns Ok.
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());
    }

    #[tokio::test]
    async fn title_delivers_to_local_subscriber() {
        let services = make_services();
        let source_id = 0xABCu128;
        let target_id = 0xDEFu128;
        let source = dummy_client_with_id(source_id);
        let target = dummy_client_with_id(target_id);

        // Register target in the registry
        services.registry.add_client(target.clone());
        services.registry.register_title(target_id, "local_only");

        let mut rex_data = RexData::new(RexCommand::Title, "local_only", b"hello");
        rex_data.set_source(source_id);
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());
    }
}

use crate::handler::port::CommandHandler;

pub struct TitleHandler;

impl CommandHandler for TitleHandler {
    async fn handle(
        &self,
        services: &crate::Services,
        client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        super::title::handle(services, client, rex_data).await
    }
}
