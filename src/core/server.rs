#[async_trait::async_trait]
pub trait RexServer {
    async fn close(&self);
}
