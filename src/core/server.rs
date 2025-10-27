#[async_trait::async_trait]
pub trait RexServer: Send + Sync {
    async fn close(&self);
}
