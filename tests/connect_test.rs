mod common;

#[cfg(test)]
mod tests {

    use std::time::Duration;

    use anyhow::Result;
    use rex_mq::protocol::RexCommand;
    use tokio::time::sleep;

    use crate::common::TestFactory;

    /**
     * 重连测试 server重启
     */
    #[tokio::test(flavor = "multi_thread")]
    async fn connect_test() -> Result<()> {
        let ss = TestFactory::default();

        let server = ss.create_server().await?;

        let mut client1 = ss.create_client("one").await?;
        let client2 = ss.create_client("").await?;

        sleep(Duration::from_secs(1)).await;

        //单播
        let a = [b'a'; 1024];
        client2.send(RexCommand::Title, "one", &a).await.unwrap();
        assert_eq!(a, client1.recv().await.unwrap().data());

        server.close().await;
        sleep(Duration::from_secs(1)).await;
        drop(server);
        sleep(Duration::from_secs(1)).await;

        let server = ss.create_server().await?;

        sleep(Duration::from_secs(2)).await;

        let a = [b'a'; 1024];
        client2.send(RexCommand::Title, "one", &a).await.unwrap();
        assert_eq!(a, client1.recv().await.unwrap().data());

        client1.close().await;
        client2.close().await;
        server.close().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }
}
