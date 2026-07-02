#[cfg(test)]
mod test_util;

mod ack;
mod cast;
mod check;
mod del_title;
mod group;
mod login;
mod reg_title;
mod title;

use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexData};
use tracing::debug;

use crate::Services;

pub async fn handle(
    services: &Services,
    client: &Arc<RexClientInner>,
    rex_data: &mut RexData,
) -> Result<()> {
    let cmd = rex_data.command();
    match cmd {
        RexCommand::Title => title::handle(services, client, rex_data).await,
        RexCommand::Group => group::handle(services, client, rex_data).await,
        RexCommand::Cast => cast::handle(services, client, rex_data).await,
        RexCommand::Login => login::handle(services, client, rex_data).await,
        RexCommand::Check => check::handle(services, client, rex_data).await,
        RexCommand::RegTitle => reg_title::handle(services, client, rex_data).await,
        RexCommand::DelTitle => del_title::handle(services, client, rex_data).await,
        RexCommand::Ack => ack::handle(services, client, rex_data).await,
        _ => {
            debug!("no handle command: {:?}", cmd);
            Ok(())
        }
    }
}
