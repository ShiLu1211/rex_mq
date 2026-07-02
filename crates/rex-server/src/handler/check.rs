use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use crate::Services;
use crate::handler::port::CommandHandler;

pub struct CheckHandler;

impl CommandHandler for CheckHandler {
    async fn handle(
        &self,
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{dummy_client_with_id, make_services};

    #[tokio::test]
    async fn check_returns_ok() {
        let services = make_services(false);
        let source = dummy_client_with_id(0xABu128);
        let mut rex_data = RexData::new(RexCommand::Check, "", b"");
        rex_data.set_source(0xABu128);
        assert!(
            CheckHandler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
    }
}
