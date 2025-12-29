use anyhow::Result;
use bytes::BytesMut;
use quinn::SendStream;
use rex_core::RexSenderTrait;
use tokio::sync::Mutex;

/// QUIC发送器，封装QUIC单向流
pub struct QuicSender {
    stream: Mutex<SendStream>,
}

impl QuicSender {
    pub fn new(stream: SendStream) -> Self {
        Self {
            stream: Mutex::new(stream),
        }
    }
}

#[async_trait::async_trait]
impl RexSenderTrait for QuicSender {
    /// 发送数据缓冲区
    async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        let mut stream = self.stream.lock().await;
        stream.write_all(buf).await?;
        Ok(())
    }

    /// 关闭连接
    async fn close(&self) -> Result<()> {
        let mut stream = self.stream.lock().await;
        stream.finish()?;
        Ok(())
    }
}
