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

#[cfg(test)]
mod tests {
    use rex_core::RexCommand;
    use rex_observability::metrics::global_registry;

    use super::*;
    use crate::handler::test_util::{dummy_client_with_id, make_services};

    /// Walk the global Prometheus registry and return the counter value
    /// for `rex_commands_total{command=X, result=Y}`. Returns 0.0 when no
    /// matching sample exists yet (label set created lazily on first inc).
    fn command_counter(command: &str, result: &str) -> f64 {
        for mf in global_registry().gather() {
            if mf.name() != "rex_commands_total" {
                continue;
            }
            for metric in mf.get_metric() {
                let mut cmd = None;
                let mut res = None;
                for p in metric.get_label() {
                    match p.name() {
                        "command" => cmd = Some(p.value()),
                        "result" => res = Some(p.value()),
                        _ => {}
                    }
                }
                if cmd == Some(command) && res == Some(result) {
                    return metric.get_counter().value();
                }
            }
        }
        0.0
    }

    /// Walk the global Prometheus registry and return the histogram sample
    /// count for `rex_command_duration_seconds{command=X}`.
    fn command_duration_count(command: &str) -> u64 {
        for mf in global_registry().gather() {
            if mf.name() != "rex_command_duration_seconds" {
                continue;
            }
            for metric in mf.get_metric() {
                for p in metric.get_label() {
                    if p.name() == "command" && p.value() == command {
                        return metric.get_histogram().get_sample_count();
                    }
                }
            }
        }
        0
    }

    #[tokio::test]
    async fn dispatch_records_ok_for_known_command() {
        let services = make_services(false);
        let source = dummy_client_with_id(0xABu128);
        let mut rex_data = RexData::new(RexCommand::Check, "", b"");
        rex_data.set_source(0xABu128);

        let before = command_counter("Check", "ok");
        handle(&services, &source, &mut rex_data)
            .await
            .expect("Check should return Ok");
        let after = command_counter("Check", "ok");

        assert!(
            after >= before + 1.0,
            "expected rex_commands_total{{command=Check,result=ok}} to increment by >= 1 (was {before}, now {after})"
        );
    }

    #[tokio::test]
    async fn dispatch_does_not_increment_err_on_success() {
        let services = make_services(false);
        let source = dummy_client_with_id(0xABu128);
        let mut rex_data = RexData::new(RexCommand::Check, "", b"");
        rex_data.set_source(0xABu128);

        let before = command_counter("Check", "err");
        handle(&services, &source, &mut rex_data)
            .await
            .expect("Check should return Ok");
        let after = command_counter("Check", "err");

        assert!(
            after <= before,
            "expected rex_commands_total{{command=Check,result=err}} to NOT increment on success (was {before}, now {after})"
        );
    }

    #[tokio::test]
    async fn dispatch_records_duration_for_known_command() {
        let services = make_services(false);
        let source = dummy_client_with_id(0xABu128);
        let mut rex_data = RexData::new(RexCommand::Check, "", b"");
        rex_data.set_source(0xABu128);

        let before = command_duration_count("Check");
        handle(&services, &source, &mut rex_data)
            .await
            .expect("Check should return Ok");
        let after = command_duration_count("Check");

        assert!(
            after > before,
            "expected rex_command_duration_seconds{{command=Check}} sample count to increment by >= 1 (was {before}, now {after})"
        );
    }
}
