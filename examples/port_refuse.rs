use std::time::Duration;

use anyhow::Result;
use tokio::time::sleep;
use tracing::info;

use rex_mq::{
    Protocol,
    protocol::{RexCommand, RexDataBuilder},
    utils::common::TestFactory,
};

#[tokio::main]
async fn main() -> Result<()> {
    port_refuse(Protocol::Tcp).await?;
    port_refuse(Protocol::Quic).await?;
    Ok(())
}

async fn port_refuse(protocol: Protocol) -> Result<()> {
    let ss: TestFactory = TestFactory::default();

    let server = ss.create_server(protocol).await?;

    let client_r = ss.create_client("one", protocol).await?;
    let client_s = ss.create_client("", protocol).await?;

    client_r.wait_for_connected().await;
    client_s.wait_for_connected().await;

    // 客户端持续接收消息（后台任务已启动）
    let count = 5;

    // 模拟用户交互：发送10条消息
    for i in 0..count {
        info!("USER: Sending message {}", i);
        let mut data = RexDataBuilder::new(RexCommand::Title)
            .title("one")
            .data_from_string(format!("Hello from client: {}", i))
            .build();
        client_s.send_data(&mut data).await?;
        sleep(Duration::from_secs(1)).await;
    }

    for i in 0..count {
        info!("USER: Sending message {}", i);
        let mut data = RexDataBuilder::new(RexCommand::Group)
            .title("one")
            .data_from_string(format!("Hello from client: {}", i))
            .build();
        client_s.send_data(&mut data).await?;
        sleep(Duration::from_secs(1)).await;
    }

    for i in 0..count {
        info!("USER: Sending message {}", i);
        let mut data = RexDataBuilder::new(RexCommand::Cast)
            .title("one")
            .data_from_string(format!("Hello from client: {}", i))
            .build();
        client_s.send_data(&mut data).await?;
        sleep(Duration::from_secs(1)).await;
    }

    info!("USER: Finished sending messages");

    // 等待一段时间让客户端接收剩余消息
    sleep(Duration::from_secs(2)).await;

    // 关闭连接
    client_s.close().await;
    client_r.close().await;
    server.close().await;

    info!("Connections closed, waiting for port release...");
    drop(client_s);
    drop(client_r);
    drop(server);
    sleep(Duration::from_secs(1)).await;

    let server = ss.create_server(protocol).await?;
    info!("port refused success");
    server.close().await;
    ss.close().await;
    sleep(Duration::from_secs(1)).await;
    Ok(())
}
