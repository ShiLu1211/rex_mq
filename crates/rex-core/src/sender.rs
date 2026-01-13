use anyhow::Result;

#[async_trait::async_trait]
pub trait RexSenderTrait: Sync + Send {
    /// send buf中实现len+payload
    async fn send_buf(&self, buf: &[u8]) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
