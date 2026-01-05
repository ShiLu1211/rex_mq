use anyhow::Result;
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
    async fn send_buf(&self, buf: &[u8]) -> Result<()> {
        let mut sink = self.sink.lock().await;

        let len = buf.len() as u32;
        let mut packet = Vec::with_capacity(4 + buf.len());

        packet.extend_from_slice(&len.to_be_bytes());
        packet.extend_from_slice(buf);

        match &mut *sink {
            WsSink::Server(s) => s.send(Message::Binary(packet.into())).await?,
            WsSink::Client(s) => s.send(Message::Binary(packet.into())).await?,
        }

        Ok(())
    }

    async fn close(&self) -> Result<()> {
        let mut sink = self.sink.lock().await;
        match &mut *sink {
            WsSink::Server(s) => s.close().await?,
            WsSink::Client(s) => s.close().await?,
        }
        Ok(())
    }
}
