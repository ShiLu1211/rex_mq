#[cfg(test)]
pub(crate) mod test_util;

mod ack;
mod cast;
mod check;
mod group;
mod login;
pub(crate) mod port;
pub(crate) mod session_mutator;
mod title;

use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use rex_observability::metrics::{
    inc_commands_total, inc_messages_failed, observe_command_duration,
};
use tracing::debug;

use crate::Services;

use port::CommandHandler;

/// Public dispatch entry point. Wraps the per-command observation around
/// the underlying [`dispatch`] so every command routed through this site
/// emits `rex_commands_total{command,result}` and
/// `rex_command_duration_seconds{command}` exactly once, regardless of
/// which `CommandHandler` impl runs.
pub async fn handle(
    services: &Services,
    client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let cmd = rex_data.command();
    let command_label = format!("{cmd:?}");
    let started = Instant::now();
    let result = dispatch(services, client, rex_data, cmd).await;
    let result_label = if result.is_ok() { "ok" } else { "err" };
    if result.is_err() {
        inc_messages_failed("handler_error");
    }
    inc_commands_total(&command_label, result_label);
    observe_command_duration(&command_label, started.elapsed().as_secs_f64());
    result
}

/// Per-command dispatch table: one handler per `RexCommand`. The table is
/// a match over the command enum rather than a `HashMap<RexCommand,
/// &dyn CommandHandler>` because the command set is small and closed —
/// adding a variant means adding a match arm, which the compiler
/// exhaustiveness check catches.
async fn dispatch(
    services: &Services,
    client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
    cmd: RexCommand,
) -> Result<()> {
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
