use anyhow::Result;
use async_trait::async_trait;
use bytes::BytesMut;
use quinn::SendStream;
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::common::Sender;

pub struct QuicSender {
    tx: Mutex<SendStream>,
}

impl QuicSender {
    pub fn new(tx: SendStream) -> Self {
        QuicSender { tx: Mutex::new(tx) }
    }
}

#[async_trait]
impl Sender for QuicSender {
    async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        let mut tx = self.tx.lock().await;
        // 框架：先写 4 字节长度（big-endian），再写 payload；不 finish，允许复用流
        let len = buf.len() as u32;
        tx.write_all(&len.to_be_bytes()).await?;
        tx.write_all(buf).await?;
        tx.flush().await?;
        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let mut tx = self.tx.lock().await;
        tx.finish()?;
        Ok(())
    }
}
