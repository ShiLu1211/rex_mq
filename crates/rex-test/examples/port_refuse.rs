use std::time::Duration;

use anyhow::Result;
use rex_core::{Protocol, RexCommand};
use rex_test::factory::TestEnv;
use strum::IntoEnumIterator;
use tokio::time::sleep;
use tracing::info;

#[tokio::main]
async fn main() -> Result<()> {
    for protocol in Protocol::iter() {
        port_refuse(protocol).await?;
    }
    Ok(())
}

async fn port_refuse(protocol: Protocol) -> Result<()> {
    let mut ss = TestEnv::default();

    let server = ss.start_server(protocol).await?;

    let client_r = ss.create_client(protocol, "one").await?;
    let client_s = ss.create_client(protocol, "").await?;

    client_r.wait_connected().await;
    client_s.wait_connected().await;

    // 客户端持续接收消息（后台任务已启动）
    let count = 5;

    // 模拟用户交互：发送10条消息
    for i in 0..count {
        info!("USER: Sending message {}", i);
        client_s
            .send(
                RexCommand::Title,
                "one",
                format!("Hello from client: {}", i).as_bytes(),
            )
            .await?;
        sleep(Duration::from_secs(1)).await;
    }

    for i in 0..count {
        info!("USER: Sending message {}", i);
        client_s
            .send(
                RexCommand::Group,
                "one",
                format!("Hello from client: {}", i).as_bytes(),
            )
            .await?;
        sleep(Duration::from_secs(1)).await;
    }

    for i in 0..count {
        info!("USER: Sending message {}", i);
        client_s
            .send(
                RexCommand::Cast,
                "one",
                format!("Hello from client: {}", i).as_bytes(),
            )
            .await?;
        sleep(Duration::from_secs(1)).await;
    }

    info!("USER: Finished sending messages");

    // 等待一段时间让客户端接收剩余消息
    sleep(Duration::from_secs(2)).await;

    // 关闭连接
    client_s.close().await;
    client_r.close().await;
    ss.close_server(protocol).await;

    info!("Connections closed, waiting for port release...");
    drop(client_s);
    drop(client_r);
    drop(server);
    sleep(Duration::from_secs(1)).await;

    let _server = ss.start_server(protocol).await?;
    info!("port refused success");
    ss.shutdown().await;
    sleep(Duration::from_secs(1)).await;
    Ok(())
}
