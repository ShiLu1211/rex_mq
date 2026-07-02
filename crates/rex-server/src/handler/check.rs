use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::Services;

pub async fn handle(
    _services: &Services,
    source_client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let client_id: u128 = rex_data.source();
    debug!("[{:032X}] Received check online", client_id);
    if let Err(e) = source_client
        .send_buf(rex_data.set_command(RexCommand::CheckReturn).pack_ref())
        .await
    {
        warn!(
            "[{:032X}] Send check return message error: {}",
            client_id, e
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::dummy_client_with_id;
    use crate::handler::test_util::{TestAckTracker, TestRegistry};
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
    async fn check_returns_ok() {
        let services = make_services();
        let source = dummy_client_with_id(0xABu128);
        let mut rex_data = RexData::new(RexCommand::Check, "", b"");
        rex_data.set_source(0xABu128);
        assert!(handle(&services, &source, &mut rex_data).await.is_ok());
    }
}

use crate::handler::port::CommandHandler;

pub struct CheckHandler;

impl CommandHandler for CheckHandler {
    async fn handle(
        &self,
        services: &crate::Services,
        client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        super::check::handle(services, client, rex_data).await
    }
}
