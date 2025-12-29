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

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    client: &Arc<RexClientInner>,
    data: &mut RexData,
) -> Result<()> {
    match data.header().command() {
        RexCommand::Title => title::handle(system, client, data).await,
        RexCommand::Group => group::handle(system, client, data).await,
        RexCommand::Cast => cast::handle(system, client, data).await,
        RexCommand::Login => login::handle(system, client, data).await,
        RexCommand::Check => check::handle(system, client, data).await,
        RexCommand::RegTitle => reg_title::handle(system, client, data).await,
        RexCommand::DelTitle => del_title::handle(system, client, data).await,
        _ => {
            debug!("no handle command: {:?}", data.header().command());
            Ok(())
        }
    }
}
