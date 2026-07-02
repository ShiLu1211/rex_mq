// src/handler/ack.rs

use std::sync::Arc;

use anyhow::Result;
use rex_core::{AckData, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::Services;

/// Handle ACK from receiver to sender
///
/// When a receiver gets a message and wants to acknowledge it,
/// the server forwards the ACK to the original sender.
pub async fn handle(
    services: &Services,
    _source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    if !services.is_ack_enabled() {
        debug!("ACK received but ACK is not enabled, ignoring");
        return Ok(());
    }

    // Parse the ACK data
    let ack_data = AckData::from_rex_data(rex_data);
    let message_id = ack_data.message_id;

    debug!("Received ACK for message: {}", message_id);

    // Look up the original sender
    if let Some(pending_ack) = services.take_pending_ack(message_id) {
        // Forward ACK to the original sender
        if let Some(sender) = services
            .registry
            .find_some_by_id(pending_ack.source_client_id)
        {
            // Create a new AckData to forward
            let ack_to_send = AckData::new(message_id);
            let rex_data =
                ack_to_send.to_rex_data(pending_ack.source_client_id, RexCommand::AckReturn);

            if let Err(e) = sender.send_buf(rex_data.pack_ref()).await {
                warn!(
                    "Failed to forward ACK to sender [{:032X}]: {}",
                    pending_ack.source_client_id, e
                );
            } else {
                debug!(
                    "ACK forwarded to sender [{:032X}]",
                    pending_ack.source_client_id
                );
            }
        } else {
            warn!(
                "Sender [{:032X}] not found for ACK message {}",
                pending_ack.source_client_id, message_id
            );
        }
    } else {
        debug!("No pending ACK found for message: {}", message_id);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{TestAckTracker, TestRegistry, dummy_client_with_id};
    use crate::{
        AckTracker, ClientRegistry, ClusterPort, NoopOfflineBuffer, OfflineBuffer, RexSystemConfig,
        Services, Shutdown,
    };
    use std::sync::Arc;

    fn make_services(ack_enabled: bool) -> Arc<Services> {
        let registry = TestRegistry::new();
        let acks = Arc::new(TestAckTracker::new()) as Arc<dyn AckTracker>;
        let offline = Arc::new(NoopOfflineBuffer) as Arc<dyn OfflineBuffer>;
        let cluster: Arc<dyn ClusterPort> =
            Arc::new(crate::handler::test_util::TestClusterPort::new());
        let shutdown = Shutdown::new();
        let mut config = RexSystemConfig::from_id("test");
        config.ack_enabled = ack_enabled;
        Services::new(registry.to_arc(), acks, offline, cluster, shutdown, config)
    }

    #[tokio::test]
    async fn ack_ignores_when_disabled() {
        let services = make_services(false);
        let source = dummy_client_with_id(1);

        let ack_data = rex_core::AckData::new(42);
        let mut rex_data = ack_data.to_rex_data(100, RexCommand::Ack);
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());
    }

    #[tokio::test]
    async fn ack_unknown_message_is_silent() {
        let services = make_services(true);
        let source = dummy_client_with_id(1);

        // Register an ACK we can satisfy, then query for a different one.
        services.acks.register(7, 200, "news".to_string(), false);

        let ack_data = rex_core::AckData::new(42); // not 7
        let mut rex_data = ack_data.to_rex_data(100, RexCommand::Ack);
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());
    }

    #[tokio::test]
    async fn ack_forward_to_sender() {
        let services = make_services(true);
        let source = dummy_client_with_id(1);
        let sender_id = 0xDECAFu128;

        // Pre-register the sender in the registry so find_some_by_id works.
        let sender_client = dummy_client_with_id(sender_id);
        services.registry.add_client(sender_client);

        // Register a pending ACK from sender_id for msg 4.
        services
            .acks
            .register(4, sender_id, "news".to_string(), false);

        let ack_data = rex_core::AckData::new(4);
        let mut rex_data = ack_data.to_rex_data(100, RexCommand::Ack);
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());

        // Pending ACK should be consumed by take.
        assert!(services.acks.take(4).is_none(), "pending ACK consumed");
    }
}

// ---- CommandHandler impl (C1) -------------------------------------------

use crate::handler::port::CommandHandler;

pub struct AckHandler;

impl CommandHandler for AckHandler {
    async fn handle(
        &self,
        services: &crate::Services,
        client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        // Delegate to the free function to keep existing tests working.
        crate::handler::ack::handle(services, client, rex_data).await
    }
}
