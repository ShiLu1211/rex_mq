#[cfg(test)]
mod tests {

    use std::time::Duration;

    use anyhow::Result;
    use rex_core::{Protocol, RetCode, RexCommand};
    use rex_test::factory::TestEnv;
    use strum::IntoEnumIterator;
    use tokio::time::sleep;

    #[tokio::test]
    async fn base_test() -> Result<()> {
        for protocol in Protocol::iter() {
            base_test_inner(protocol).await?;
        }
        Ok(())
    }

    async fn base_test_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::default();

        let server = ss.start_server(protocol).await?;

        let mut client1 = ss.create_client(protocol, "hello;bcd").await?;
        let mut client2 = ss.create_client(protocol, "hello;abc").await?;
        let mut client3 = ss.create_client(protocol, "hello;abc").await?;

        client1.wait_connected().await;
        client2.wait_connected().await;
        client3.wait_connected().await;

        //目标地址不可达
        client1
            .send(RexCommand::Title, "abc999", &[b'a'; 1024])
            .await
            .unwrap();
        assert_eq!(
            RetCode::NoTargetAvailable,
            client1.recv().await.unwrap().retcode()
        );

        // 大数据测试
        let a = vec![1; 8192 * 10];
        client1.send(RexCommand::Title, "abc", &a).await.unwrap();

        let recv_data = tokio::select! {
            data = client2.recv() => data.unwrap(),
            data = client3.recv() => data.unwrap(),
        };
        assert_eq!(a, recv_data.data().to_vec());

        //单播
        let a = [b'a'; 1024];
        client1.send(RexCommand::Title, "abc", &a).await.unwrap();

        let recv_data = tokio::select! {
            data = client2.recv() => data.unwrap(),
            data = client3.recv() => data.unwrap(),
        };
        assert_eq!(a, recv_data.data());

        //组播
        let a = [b'b'; 1024];
        client1.send(RexCommand::Group, "abc", &a).await.unwrap();
        let recv_data = tokio::select! {
            data = client2.recv() => data.unwrap(),
            data = client3.recv() => data.unwrap(),
        };
        assert_eq!(a, recv_data.data());

        //广播
        let a = [b'c'; 1024];
        client1.send(RexCommand::Cast, "hello", &a).await.unwrap();
        assert_eq!(a, client2.recv().await.unwrap().data());
        assert_eq!(a, client3.recv().await.unwrap().data());

        client1.close().await;
        client2.close().await;
        client3.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }
}
