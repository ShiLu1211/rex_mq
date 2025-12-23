use anyhow::Result;
use bytes::BytesMut;
use tokio::sync::mpsc;

pub enum WriteCommand {
    Data(BytesMut),
    Close,
}

pub struct RexSender {
    tx: mpsc::Sender<WriteCommand>,
}

impl RexSender {
    // 构造函数传入通道的 Sender
    pub fn new(tx: mpsc::Sender<WriteCommand>) -> Self {
        Self { tx }
    }
}

impl RexSender {
    pub async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        // 克隆 buffer 是为了发送所有权，BytesMut 的 clone 开销很小（浅拷贝）
        // 如果通道已满或接收端已关闭，这里会返回错误
        self.tx
            .send(WriteCommand::Data(buf.clone()))
            .await
            .map_err(|_| anyhow::anyhow!("Connection closed (send channel failed)"))?;
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        // 发送关闭信号
        let _ = self.tx.send(WriteCommand::Close).await;
        Ok(())
    }
}
