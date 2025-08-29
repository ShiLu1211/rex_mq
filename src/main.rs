mod quic_client;
mod quic_server;

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::Result;
use tokio::time::sleep;
use tracing::info;

use crate::{quic_client::MyQuicClient, quic_server::MyQuicServer};

#[tokio::main]
async fn main() -> Result<()> {
    // 初始化日志
    tracing_subscriber::fmt::init();

    let port = 8881;
    let server_addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)), port);

    // 启动服务器
    let server = MyQuicServer::open(server_addr).await?;
    info!("Server started on {}", server_addr);

    sleep(Duration::from_secs(1)).await;

    // 创建客户端（自动启动接收任务）
    let client = MyQuicClient::create(server_addr).await?;
    info!("Client connected to server");

    // 客户端持续接收消息（后台任务已启动）

    // 模拟用户交互：发送10条消息
    for i in 0..10 {
        info!("USER: Sending message {}", i);
        client.send(&format!("Hello from client: {}", i)).await?;
        sleep(Duration::from_secs(1)).await;
    }

    info!("USER: Finished sending messages");

    // 等待一段时间让客户端接收剩余消息
    sleep(Duration::from_secs(2)).await;

    // 关闭连接
    client.close().await;
    sleep(Duration::from_secs(1)).await;
    server.close().await;

    info!("Connections closed, waiting for port release...");
    sleep(Duration::from_secs(15)).await;

    let _server = MyQuicServer::open(server_addr).await?;

    Ok(())
}
