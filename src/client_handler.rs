use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;

use crate::{client::RexClient, data::RexData};

#[async_trait]
pub trait RexClientHandler: Send + Sync {
    async fn login_ok(&self, client: Arc<RexClient>, data: &RexData) -> Result<()>;
    async fn handle(&self, client: Arc<RexClient>, data: &RexData) -> Result<()>;
}
