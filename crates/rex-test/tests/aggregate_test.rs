#[cfg(test)]
mod tests {
    use anyhow::Result;
    use rex_core::{Protocol, RexCommand};
    use rex_test::factory::TestEnv;

    #[tokio::test]
    async fn aggregate_test() -> Result<()> {
        let mut env = TestEnv::default();

        // 启动三种协议
        env.start_aggregate_server(&[Protocol::Tcp, Protocol::Quic, Protocol::WebSocket])
            .await?;

        // 创建对应 client
        let mut tcp_client = env.create_client(Protocol::Tcp, "tcp;one").await?;
        tcp_client.wait_connected().await;
        let mut quic_client = env.create_client(Protocol::Quic, "quic;one").await?;
        quic_client.wait_connected().await;
        let mut ws_client = env.create_client(Protocol::WebSocket, "ws;one").await?;
        ws_client.wait_connected().await;

        let test_data = "message from tcp".as_bytes();
        tcp_client.send(RexCommand::Cast, "one", test_data).await?;
        let data = quic_client.recv().await.unwrap();
        assert_eq!(test_data, data.data());
        let data = ws_client.recv().await.unwrap();
        assert_eq!(test_data, data.data());

        let test_data = "message from quic".as_bytes();
        quic_client.send(RexCommand::Cast, "one", test_data).await?;
        let data = tcp_client.recv().await.unwrap();
        assert_eq!(test_data, data.data());
        let data = ws_client.recv().await.unwrap();
        assert_eq!(test_data, data.data());

        ws_client.close().await;
        tcp_client.close().await;
        env.shutdown().await;
        Ok(())
    }
}
