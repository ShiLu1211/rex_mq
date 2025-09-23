use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use anyhow::Result;
use async_trait::async_trait;
use tokio::time::sleep;
use tracing::info;

use rex_mq::{
    QuicClient, QuicServer, RexClient, RexClientHandler, RexCommand, RexData, RexDataBuilder,
};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    let port = 8881;
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);

    // 启动服务器
    let server = QuicServer::open(server_addr).await?;
    info!("Server started on {}", server_addr);

    sleep(Duration::from_secs(1)).await;

    // 创建客户端（自动启动接收任务）
    let client_r = QuicClient::new(server_addr, "one".into(), Arc::new(RcvClientHandler)).await?;
    let client_r = client_r.open().await?;
    info!("Client connected to server");

    let client_s = QuicClient::new(server_addr, "".into(), Arc::new(SndClientHandler)).await?;
    let client_s = client_s.open().await?;
    info!("Client connected to server");

    // 客户端持续接收消息（后台任务已启动）

    // 模拟用户交互：发送10条消息
    for i in 0..10 {
        info!("USER: Sending message {}", i);
        let mut data = RexDataBuilder::new(RexCommand::Title)
            .title("one")
            .data_from_slice(format!("Hello from client: {}", i).as_bytes())
            .build();
        client_s.send_data(&mut data).await?;
        sleep(Duration::from_secs(1)).await;
    }

    for i in 0..10 {
        info!("USER: Sending message {}", i);
        let mut data = RexDataBuilder::new(RexCommand::Group)
            .title("one")
            .data_from_slice(format!("Hello from client: {}", i).as_bytes())
            .build();
        client_s.send_data(&mut data).await?;
        sleep(Duration::from_secs(1)).await;
    }

    for i in 0..10 {
        info!("USER: Sending message {}", i);
        let mut data = RexDataBuilder::new(RexCommand::Cast)
            .title("one")
            .data_from_slice(format!("Hello from client: {}", i).as_bytes())
            .build();
        client_s.send_data(&mut data).await?;
        sleep(Duration::from_secs(1)).await;
    }

    info!("USER: Finished sending messages");

    // 等待一段时间让客户端接收剩余消息
    sleep(Duration::from_secs(2)).await;

    // 关闭连接
    client_s.close().await;
    sleep(Duration::from_secs(1)).await;
    client_r.close().await;
    sleep(Duration::from_secs(1)).await;
    server.close().await;

    info!("Connections closed, waiting for port release...");
    sleep(Duration::from_secs(1)).await;
    drop(client_s);
    drop(client_r);
    drop(server);
    sleep(Duration::from_secs(1)).await;

    let _server = QuicServer::open(server_addr).await?;
    sleep(Duration::from_secs(1)).await;
    info!("port refused success");
    Ok(())
}

struct RcvClientHandler;

#[async_trait]
impl RexClientHandler for RcvClientHandler {
    async fn login_ok(&self, client: Arc<RexClient>, _data: &RexData) -> Result<()> {
        info!("RcvHandler: Login OK for client ID {}", client.id());
        Ok(())
    }

    async fn handle(&self, client: Arc<RexClient>, data: &RexData) -> Result<()> {
        info!(
            "RcvHandler: Received data for client ID {}: {:?}",
            client.id(),
            data.data()
        );
        Ok(())
    }
}

struct SndClientHandler;

#[async_trait]
impl RexClientHandler for SndClientHandler {
    async fn login_ok(&self, client: Arc<RexClient>, _data: &RexData) -> Result<()> {
        info!("SndHandler: Login OK for client ID {}", client.id());
        Ok(())
    }

    async fn handle(&self, client: Arc<RexClient>, data: &RexData) -> Result<()> {
        info!(
            "SndHandler: Received data for client ID {}: {:?}",
            client.id(),
            data.data()
        );
        Ok(())
    }
}
