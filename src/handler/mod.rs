mod title_handler;

use std::sync::Arc;

use anyhow::Result;
use tracing::debug;

use crate::{protocol::{RexCommand, RexData}, RexClientInner, RexSystem};

pub async fn handler(
    system: &Arc<RexSystem>,
    client: &Arc<RexClientInner>,
    mut data: RexData,
) -> Result<()> {
    match data.header().command() {
        RexCommand::Title => title_handler::handle(system, client, data).await,
        _ => {
            debug!("no handle command: {:?}", data.header().command());
            Ok(())
        }
    }
}
