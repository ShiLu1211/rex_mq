mod client;
mod client_handler;
mod command;
mod common;
mod data;
mod handler;
mod quic_client;
mod quic_sender;
mod quic_server;
mod sender;

use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

use anyhow::Result;
use tokio::time::sleep;
use tracing::info;

use crate::{
    command::RexCommand, data::RexDataBuilder, quic_client::QuicClient, quic_server::QuicServer,
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
    let client_r = QuicClient::create(server_addr, "one".into()).await?;
    info!("Client connected to server");

    let client_s = QuicClient::create(server_addr, "".into()).await?;
    info!("Client connected to server");

    // 客户端持续接收消息（后台任务已启动）

    // 模拟用户交互：发送10条消息
    for i in 0..10 {
        info!("USER: Sending message {}", i);
        let data = RexDataBuilder::new(RexCommand::Title)
            .title("one")
            .data_from_slice(format!("Hello from client: {}", i).as_bytes())
            .build();
        client_s.send(&data.serialize()).await?;
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
    sleep(Duration::from_secs(5)).await;
    drop(client_s);
    drop(client_r);
    drop(server);
    sleep(Duration::from_secs(5)).await;

    let _server = QuicServer::open(server_addr).await?;

    Ok(())
}
