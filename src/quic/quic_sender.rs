use anyhow::Result;
use bytes::BytesMut;
use quinn::SendStream;
use tokio::{io::AsyncWriteExt, sync::Mutex};

use crate::RexSender;

pub struct QuicSender {
    tx: Mutex<SendStream>,
}

impl QuicSender {
    pub fn new(tx: SendStream) -> Self {
        QuicSender { tx: Mutex::new(tx) }
    }
}

#[async_trait::async_trait]
impl RexSender for QuicSender {
    async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        let mut tx = self.tx.lock().await;
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
