use anyhow::Result;
use bytes::BytesMut;
use futures_util::SinkExt;
use futures_util::stream::SplitSink;
use rex_core::RexSenderTrait;
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;

// 支持服务端（TcpStream）和客户端（MaybeTlsStream）
enum WsSink {
    Server(SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>),
    Client(
        SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    ),
}

/// WebSocket发送器，封装WebSocket写入流
pub struct WebSocketSender {
    sink: Mutex<WsSink>,
}

impl WebSocketSender {
    pub fn new_server(
        sink: SplitSink<tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>, Message>,
    ) -> Self {
        Self {
            sink: Mutex::new(WsSink::Server(sink)),
        }
    }

    pub fn new_client(
        sink: SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
    ) -> Self {
        Self {
            sink: Mutex::new(WsSink::Client(sink)),
        }
    }
}

#[async_trait::async_trait]
impl RexSenderTrait for WebSocketSender {
    /// 发送数据缓冲区
    async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        let mut sink = self.sink.lock().await;
        match &mut *sink {
            WsSink::Server(s) => s.send(Message::Binary(buf.clone().freeze())).await?,
            WsSink::Client(s) => s.send(Message::Binary(buf.clone().freeze())).await?,
        }
        Ok(())
    }

    /// 关闭连接
    async fn close(&self) -> Result<()> {
        let mut sink = self.sink.lock().await;
        match &mut *sink {
            WsSink::Server(s) => s.close().await?,
            WsSink::Client(s) => s.close().await?,
        }
        Ok(())
    }
}
