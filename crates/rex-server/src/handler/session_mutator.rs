//! Title-registration and title-unregistration have the same shape:
//!
//! 1. Look up the source client in the registry.
//! 2. Mutate the title subscription (register or unregister).
//! 3. Broadcast the change to the cluster.
//! 4. Send the return packet (RegTitleReturn or DelTitleReturn).
//! 5. On error, send a RetCode::NoTarget back to the source.
//!
//! Before C1 these lived in two files (`reg_title.rs`, `del_title.rs`).
//! After C1 they are one `SessionMutator` struct implementing the
//! `CommandHandler` trait for both `RexCommand` variants.

use std::sync::Arc;

use anyhow::Result;
use rex_cluster::types::{ClusterMessage, TitleRegisterMessage, TitleUnregisterMessage};
use rex_core::{RetCode, RexClientInner, RexCommand, RexData};
use tracing::{debug, warn};

use super::port::CommandHandler;
use crate::Services;

/// Mutation direction — encodes which registry method and which cluster
/// message to use.
enum Mutation {
    Register,
    Unregister,
}

impl Mutation {
    fn from_command(cmd: RexCommand) -> Option<Self> {
        match cmd {
            RexCommand::RegTitle => Some(Self::Register),
            RexCommand::DelTitle => Some(Self::Unregister),
            _ => None,
        }
    }

    fn return_command(&self) -> RexCommand {
        match self {
            Self::Register => RexCommand::RegTitleReturn,
            Self::Unregister => RexCommand::DelTitleReturn,
        }
    }
}

pub struct SessionMutator;

impl CommandHandler for SessionMutator {
    async fn handle(
        &self,
        services: &Services,
        source_client: &Arc<RexClientInner>,
        rex_data: &mut RexData,
    ) -> Result<()> {
        let client_id = rex_data.source();
        let title = rex_data.title().to_string();
        let mutation = Mutation::from_command(rex_data.command()).ok_or_else(|| {
            anyhow::anyhow!(
                "SessionMutator invoked with non-mutator command: {:?}",
                rex_data.command()
            )
        })?;

        debug!(
            "[{:032X}] Received {:?} [{}]",
            client_id,
            mutation.return_command(),
            title
        );

        if let Some(client) = services.registry.find_some_by_id(client_id) {
            match mutation {
                Mutation::Register => services.registry.register_title(client_id, &title),
                Mutation::Unregister => services.registry.unregister_title(client_id, &title),
            }

            // Broadcast the mutation to the cluster.
            let local_id = services.cluster.get_local_node_id().unwrap_or_default();
            let msg = match mutation {
                Mutation::Register => ClusterMessage::TitleRegister(TitleRegisterMessage {
                    node_id: local_id,
                    title: title.clone(),
                    client_id,
                }),
                Mutation::Unregister => ClusterMessage::TitleUnregister(TitleUnregisterMessage {
                    node_id: local_id,
                    title: title.clone(),
                }),
            };
            let _ = services.cluster.broadcast(msg).await;

            if let Err(e) = client
                .send_buf(rex_data.set_command(mutation.return_command()).pack_ref())
                .await
            {
                warn!(
                    "[{:032X}] Send {:?} error: {}",
                    client_id,
                    mutation.return_command(),
                    e
                );
            }
        } else {
            if let Err(e) = source_client
                .send_buf(
                    rex_data
                        .set_command(mutation.return_command())
                        .set_retcode(RetCode::NoTarget)
                        .pack_ref(),
                )
                .await
            {
                warn!(
                    "[{:032X}] Send {:?} error: {}",
                    client_id,
                    mutation.return_command(),
                    e
                );
            }
        }
        Ok(())
    }
}

// ---- Tests ----------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handler::test_util::{dummy_client_with_id, make_services};

    #[tokio::test]
    async fn reg_title_registers_and_broadcasts() {
        let services = make_services(false);
        let id = 0xBADu128;
        let source = dummy_client_with_id(id);

        // Pre-add the client so find_some_by_id works.
        services.registry.add_client(source.clone());

        let mut rex_data = RexData::new(RexCommand::RegTitle, "news", b"");
        rex_data.set_source(id);

        let handler = SessionMutator;
        assert!(
            handler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
        assert!(services.registry.find_one_by_title("news", None).is_some());
    }

    #[tokio::test]
    async fn del_title_unregisters_and_broadcasts() {
        let services = make_services(false);
        let id = 0xBADu128;
        let source = dummy_client_with_id(id);
        services.registry.add_client(source.clone());
        services.registry.register_title(id, "news");

        let mut rex_data = RexData::new(RexCommand::DelTitle, "news", b"");
        rex_data.set_source(id);

        let handler = SessionMutator;
        assert!(
            handler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
        assert!(services.registry.find_one_by_title("news", None).is_none());
    }

    #[tokio::test]
    async fn session_mutator_unknown_client_returns_no_target() {
        let services = make_services(false);
        let source = dummy_client_with_id(0xDEADu128);

        let mut rex_data = RexData::new(RexCommand::RegTitle, "news", b"");
        rex_data.set_source(0xDEADu128);

        let handler = SessionMutator;
        assert!(
            handler
                .handle(&services, &source, &mut rex_data)
                .await
                .is_ok()
        );
    }
}
