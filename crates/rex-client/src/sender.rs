use anyhow::Result;
use bytes::BytesMut;

#[async_trait::async_trait]
pub trait RexSenderTrait: Sync + Send {
    async fn send_buf(&self, buf: &BytesMut) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
