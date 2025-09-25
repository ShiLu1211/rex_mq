use std::sync::Arc;

use anyhow::Result;

use crate::RexServerConfig;

#[async_trait::async_trait]
pub trait RexServer {
    async fn open(config: RexServerConfig) -> Result<Arc<Self>>;
    async fn close(&self);
}
