mod cast_handler;
mod check_handler;
mod del_title_handler;
mod group_handler;
mod login_handler;
mod reg_title_handler;
mod title_handler;

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
        RexCommand::Title => title_handler::handle(system, client, data).await,
        RexCommand::Group => group_handler::handle(system, client, data).await,
        RexCommand::Cast => cast_handler::handle(system, client, data).await,
        RexCommand::Login => login_handler::handle(system, client, data).await,
        RexCommand::Check => check_handler::handle(system, client, data).await,
        RexCommand::RegTitle => reg_title_handler::handle(system, client, data).await,
        RexCommand::DelTitle => del_title_handler::handle(system, client, data).await,
        _ => {
            debug!("no handle command: {:?}", data.header().command());
            Ok(())
        }
    }
}
