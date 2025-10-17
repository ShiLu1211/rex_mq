mod common;

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use anyhow::Result;
    use rex_mq::protocol::RexCommand;
    use tokio::time::sleep;

    use crate::common::{Protocol, TestFactory};

    #[tokio::test]
    async fn connect_test() -> Result<()> {
        // connect_test_inner(Protocol::Tcp).await?;
        connect_test_inner(Protocol::Quic).await?;
        Ok(())
    }
    /**
     * 重连测试 server重启
     */
    async fn connect_test_inner(protocol: Protocol) -> Result<()> {
        let ss = TestFactory::default();

        let server = ss.create_server(protocol).await?;

        let mut client1 = ss.create_client("one", protocol).await?;
        let client2 = ss.create_client("", protocol).await?;

        sleep(Duration::from_secs(1)).await;

        //单播
        let a = [b'a'; 1024];
        client2.send(RexCommand::Title, "one", &a).await.unwrap();
        assert_eq!(a, client1.recv().await.unwrap().data());

        server.close().await;
        drop(server);
        sleep(Duration::from_secs(16)).await;

        let server = ss.create_server(protocol).await?;

        sleep(Duration::from_secs(15)).await;

        let a = [b'a'; 1024];
        client2.send(RexCommand::Title, "one", &a).await.unwrap();
        assert_eq!(a, client1.recv().await.unwrap().data());

        client1.close().await;
        client2.close().await;
        server.close().await;
        ss.close().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }
}
