#[cfg(test)]
mod test_util;

mod ack;
mod cast;
mod check;
mod group;
mod login;
pub(crate) mod port;
pub(crate) mod session_mutator;
mod title;

use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::debug;

use crate::Services;

use port::CommandHandler;

/// Dispatch table: one handler per `RexCommand`. The table is a match over
/// the command enum rather than a `HashMap<RexCommand, &dyn CommandHandler>`
/// because the command set is small and closed — adding a variant means
/// adding a match arm, which the compiler exhaustiveness check catches.
pub async fn handle(
    services: &Services,
    client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let cmd = rex_data.command();
    match cmd {
        RexCommand::Title => title::TitleHandler.handle(services, client, rex_data).await,
        RexCommand::Group => group::GroupHandler.handle(services, client, rex_data).await,
        RexCommand::Cast => cast::CastHandler.handle(services, client, rex_data).await,
        RexCommand::Login => login::LoginHandler.handle(services, client, rex_data).await,
        RexCommand::Check => check::CheckHandler.handle(services, client, rex_data).await,
        // C1: reg_title + del_title collapsed into SessionMutator
        RexCommand::RegTitle => {
            session_mutator::SessionMutator
                .handle(services, client, rex_data)
                .await
        }
        RexCommand::DelTitle => {
            session_mutator::SessionMutator
                .handle(services, client, rex_data)
                .await
        }
        RexCommand::Ack => ack::AckHandler.handle(services, client, rex_data).await,
        _ => {
            debug!("no handle command: {:?}", cmd);
            Ok(())
        }
    }
}
