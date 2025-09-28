use std::sync::Arc;

use anyhow::Result;

use crate::{RexServerConfig, RexSystem};

#[async_trait::async_trait]
pub trait RexServer {
    async fn open(system: Arc<RexSystem>, config: RexServerConfig) -> Result<Arc<Self>>;
    async fn close(&self);
}
