use std::sync::Arc;

use anyhow::Result;
use rex_core::{RexClientInner, RexData};

#[async_trait::async_trait]
pub trait RexClientHandlerTrait: Send + Sync {
    async fn login_ok(&self, client: Arc<RexClientInner>, data: RexData) -> Result<()>;
    async fn handle(&self, client: Arc<RexClientInner>, data: RexData) -> Result<()>;
}
