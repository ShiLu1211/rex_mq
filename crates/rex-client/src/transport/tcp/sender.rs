use anyhow::Result;
use bytes::BytesMut;
use tokio::{io::AsyncWriteExt, net::tcp::OwnedWriteHalf, sync::Mutex};

use crate::RexSenderTrait;

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
    async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer.write_all(buf).await?;
        Ok(())
    }

    /// 关闭连接
    async fn close(&self) -> Result<()> {
        let mut writer = self.writer.lock().await;
        writer.shutdown().await?;
        Ok(())
    }
}
