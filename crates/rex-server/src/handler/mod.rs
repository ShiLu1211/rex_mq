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
    data_bytes: &mut [u8],
) -> Result<()> {
    let data = RexData::as_archive(data_bytes);
    let cmd = RexCommand::from_u32(data.header.command.into());
    match cmd {
        RexCommand::Title => title::handle(system, client, data_bytes).await,
        RexCommand::Group => group::handle(system, client, data_bytes).await,
        RexCommand::Cast => cast::handle(system, client, data_bytes).await,
        RexCommand::Login => login::handle(system, client, data_bytes).await,
        RexCommand::Check => check::handle(system, client, data_bytes).await,
        RexCommand::RegTitle => reg_title::handle(system, client, data_bytes).await,
        RexCommand::DelTitle => del_title::handle(system, client, data_bytes).await,
        _ => {
            debug!("no handle command: {:?}", cmd);
            Ok(())
        }
    }
}
