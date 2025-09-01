use anyhow::Result;
use async_trait::async_trait;
use bytes::BytesMut;

#[async_trait]
pub trait Sender: Sync + Send {
    async fn send_buf(&self, buf: &BytesMut) -> Result<()>;
    async fn close(&self) -> Result<()>;
}
