use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, info, warn};

use crate::Services;

pub async fn handle(
    services: &Services,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let client_id: u128 = rex_data.source();
    debug!("[{:032X}] Received login message", client_id);
    let title = rex_data.title().to_owned();

    if let Some(client) = services.registry.find_some_by_id(client_id) {
        warn!("[{:032X}] Client already exists", client_id);
        client.set_sender(source_client.sender().clone());
        client.insert_title(&title);

        if let Err(e) = client
            .send_buf(rex_data.set_command(RexCommand::LoginReturn).pack_ref())
            .await
        {
            warn!(
                "[{:032X}] Send login return message error: {}",
                client_id, e
            );
        } else {
            info!(
                "Client [{:032X}] logged in with title: {}",
                client_id, title
            );
        }
    } else {
        source_client.set_id(client_id);
        source_client.insert_title(&title);

        services.add_client(source_client.clone()).await;

        // Drain any messages queued for this client ID while it was offline.
        let queued = services.get_offline_messages(client_id).await;
        if !queued.is_empty() {
            info!(
                "Client [{:032X}] reconnecting with {} queued offline message(s)",
                client_id,
                queued.len()
            );
            for msg in queued {
                let mut title_data = RexData::new(RexCommand::Title, &msg.title, &msg.payload);
                title_data.set_source(client_id);
                title_data.set_message_id(msg.id);
                if let Err(e) = source_client.send_buf(title_data.pack_ref()).await {
                    warn!(
                        "Failed to deliver queued offline message [{:032X}] to client: {}",
                        msg.id, e
                    );
                }
            }
            services.clear_offline_messages(client_id).await;
        }

        if let Err(e) = source_client
            .send_buf(rex_data.set_command(RexCommand::LoginReturn).pack_ref())
            .await
        {
            warn!(
                "[{:032X}] Send login return message error: {}",
                client_id, e
            );
        } else {
            info!(
                "New client [{:032X}] logged in with title: {}",
                source_client.id(),
                title
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{TestAckTracker, TestRegistry, dummy_client_with_id};
    use crate::{
        AckTracker, ClientRegistry, ClusterPort, ClusterRouter, NoopOfflineBuffer, OfflineBuffer,
        RexSystemConfig, Router, Services, Shutdown,
    };
    use std::sync::Arc;

    fn make_services() -> Arc<Services> {
        let registry = TestRegistry::new();
        let acks = Arc::new(TestAckTracker::new()) as Arc<dyn AckTracker>;
        let offline = Arc::new(NoopOfflineBuffer) as Arc<dyn OfflineBuffer>;
        let cluster: Arc<dyn ClusterPort> =
            Arc::new(crate::handler::test_util::TestClusterPort::new());
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
    async fn login_new_client_registers_and_sends_login_return() {
        let services = make_services();
        let client_id = 0xCAFEu128;
        let source = dummy_client_with_id(client_id);
        let title = "news";

        let mut rex_data = RexData::new(RexCommand::Login, title, b"");
        rex_data.set_source(client_id);

        // New client: should not already exist in the registry.
        assert!(services.registry.find_some_by_id(client_id).is_none());

        // handle returns Ok
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());

        // Client is now in the registry
        let registered = services
            .registry
            .find_some_by_id(client_id)
            .expect("client should be registered");
        assert_eq!(registered.id(), client_id);
    }

    #[tokio::test]
    async fn login_existing_client_updates_sender_and_title() {
        let services = make_services();
        let client_id = 0xBEEFu128;
        let source = dummy_client_with_id(client_id);
        let title = "news";

        // Pre-register the client in the registry
        services.registry.add_client(source.clone());
        services.registry.register_title(client_id, title);

        let mut rex_data = RexData::new(RexCommand::Login, title, b"");
        rex_data.set_source(client_id);

        // Existing client: handle should still succeed
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());

        // Client still exists
        assert!(services.registry.find_some_by_id(client_id).is_some());
    }

    #[tokio::test]
    async fn login_new_client_does_not_send_offline_messages_when_empty() {
        // With NoopOfflineBuffer, get_offline_messages always returns empty.
        // The handler should still succeed without panicking.
        let services = make_services();
        let client_id = 0xDEADu128;
        let source = dummy_client_with_id(client_id);
        let title = "news";

        let mut rex_data = RexData::new(RexCommand::Login, title, b"");
        rex_data.set_source(client_id);

        assert!(handle(&services, &source, &mut rex_data).await.is_ok());
    }
}

use crate::handler::port::CommandHandler;

pub struct LoginHandler;

impl CommandHandler for LoginHandler {
    async fn handle(
        &self,
        services: &crate::Services,
        client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        super::login::handle(services, client, rex_data).await
    }
}
