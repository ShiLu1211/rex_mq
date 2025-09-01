use anyhow::Result;
use async_trait::async_trait;
use bytes::BytesMut;

#[async_trait]
pub trait RexSender: Sync + Send {
    async fn send_buf(&self, buf: &BytesMut) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
