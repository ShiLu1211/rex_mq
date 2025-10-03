mod cast;
mod check;
mod del_title;
mod group;
mod login;
mod reg;
mod title;

use std::sync::Arc;

use anyhow::Result;
use tracing::debug;

use crate::{
    RexClientInner, RexSystem,
    protocol::{RexCommand, RexData},
};

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
        RexCommand::RegTitle => reg::handle(system, client, data).await,
        RexCommand::DelTitle => del_title::handle(system, client, data).await,
        _ => {
            debug!("no handle command: {:?}", data.header().command());
            Ok(())
        }
    }
}
