mod cast;
mod check;
mod del_title;
mod group;
mod login;
mod reg_title;
mod title;

use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexCommand, RexDataRef};
use tracing::debug;

use crate::RexSystem;

pub async fn handle(
    system: &Arc<RexSystem>,
    client: &Arc<RexClientInner>,
    data_ref: RexDataRef<'_>, // 零拷贝访问
) -> Result<()> {
    match data_ref.command() {
        RexCommand::Title => title::handle(system, client, data_ref).await,
        RexCommand::Group => group::handle(system, client, data_ref).await,
        RexCommand::Cast => cast::handle(system, client, data_ref).await,
        RexCommand::Login => login::handle(system, client, data_ref).await,
        RexCommand::Check => check::handle(system, client, data_ref).await,
        RexCommand::RegTitle => reg_title::handle(system, client, data_ref).await,
        RexCommand::DelTitle => del_title::handle(system, client, data_ref).await,
        _ => {
            debug!("no handle command: {:?}", data_ref.command());
            Ok(())
        }
    }
}
