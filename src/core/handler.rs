use std::sync::Arc;

use anyhow::Result;

use crate::RexClientInner;
use crate::protocol::RexData;

#[async_trait::async_trait]
pub trait RexClientHandler: Send + Sync {
    async fn login_ok(&self, client: Arc<RexClientInner>, data: &RexData) -> Result<()>;
    async fn handle(&self, client: Arc<RexClientInner>, data: &RexData) -> Result<()>;
}
