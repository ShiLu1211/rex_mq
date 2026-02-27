#[cfg(test)]
mod tests {

    use std::time::Duration;

    use anyhow::Result;
    use rex_core::{Protocol, RexCommand};
    use rex_test::factory::TestEnv;
    use strum::IntoEnumIterator;
    use tokio::time::sleep;

    #[tokio::test]
    async fn connect_test() -> Result<()> {
        for protocol in Protocol::iter() {
            connect_test_inner(protocol).await?;
        }
        Ok(())
    }
    /**
     * 重连测试 server重启
     */
    async fn connect_test_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new().await;

        let server = ss.start_server(protocol).await?;

        let mut client1 = ss.create_client(protocol, "one").await?;
        let client2 = ss.create_client(protocol, "").await?;

        client1.wait_connected().await;
        client2.wait_connected().await;

        //单播
        let a = [b'a'; 1024];
        client2.send(RexCommand::Title, "one", &a).await.unwrap();
        assert_eq!(a, client1.recv().await.unwrap().data());

        // 关闭服务器并获取地址，用于重启同一端口
        let server_addr = ss.close_server(protocol).await;
        drop(server);
        sleep(Duration::from_secs(1)).await;

        // 重启服务器在同一端口
        if let Some(addr) = server_addr {
            ss.start_server_with_addr(protocol, addr).await?;
        } else {
            ss.start_server(protocol).await?;
        }

        client1.wait_connected().await;
        client2.wait_connected().await;

        let a = [b'a'; 1024];
        client2.send(RexCommand::Title, "one", &a).await.unwrap();
        assert_eq!(a, client1.recv().await.unwrap().data());

        client1.close().await;
        client2.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }
}
