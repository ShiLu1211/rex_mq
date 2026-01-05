use anyhow::Result;
use rex_core::RexSenderTrait;
use tokio::{io::AsyncWriteExt, net::tcp::OwnedWriteHalf, sync::Mutex};

/// TCP发送器，封装TCP写入流
pub struct TcpSender {
    writer: Mutex<OwnedWriteHalf>,
}

impl TcpSender {
    pub fn new(writer: OwnedWriteHalf) -> Self {
        Self {
            writer: Mutex::new(writer),
        }
    }
}

#[async_trait::async_trait]
impl RexSenderTrait for TcpSender {
    /// 发送数据缓冲区
    async fn send_buf(&self, buf: &[u8]) -> Result<()> {
        let mut writer = self.writer.lock().await;

        let len = buf.len() as u32;
        let mut packet = Vec::with_capacity(4 + buf.len());

        packet.extend_from_slice(&len.to_be_bytes());
        packet.extend_from_slice(buf);

        writer.write_all(&packet).await?;
        Ok(())
    }

    /// 关闭连接
    async fn close(&self) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer.shutdown().await?;
        Ok(())
    }
}
