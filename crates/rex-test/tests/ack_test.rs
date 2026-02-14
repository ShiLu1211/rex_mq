#[cfg(test)]
mod tests {
    use std::time::Duration;

    use anyhow::Result;
    use rex_core::{Protocol, RetCode, RexCommand};
    use rex_test::factory::TestEnv;
    use serial_test::serial;
    use tokio::time::sleep;

    /// Test ACK functionality with Title (unicast) message - TCP only
    #[tokio::test]
    #[serial]
    async fn test_ack_title_tcp() -> Result<()> {
        test_ack_title_inner(Protocol::Tcp).await
    }

    async fn test_ack_title_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new_with_ack().await;

        let server = ss.start_server(protocol).await?;

        // Create sender first
        let mut sender = ss.create_client_with_ack(protocol, "sender").await?;
        sender.wait_connected().await;

        // Create receiver and wait for it to connect
        let mut receiver = ss.create_client_with_ack(protocol, "test-title").await?;
        receiver.wait_connected().await;

        // Small delay to ensure both clients are fully registered
        sleep(Duration::from_millis(200)).await;

        // Send a message with ACK
        let test_data = b"Hello, ACK test!";
        sender
            .send(RexCommand::Title, "test-title", test_data)
            .await?;

        // Receiver should receive the message
        let recv_data = receiver.recv().await.expect("Should receive message");
        assert_eq!(test_data, recv_data.data());

        // Sender should receive ACK return
        let ack_data = sender.recv().await.expect("Should receive ACK");
        assert_eq!(RexCommand::AckReturn, ack_data.command());
        assert_eq!(RetCode::Success, ack_data.retcode());

        sender.close().await;
        receiver.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    /// Test ACK functionality with Group (multicast) message - TCP only
    #[tokio::test]
    #[serial]
    async fn test_ack_group_tcp() -> Result<()> {
        test_ack_group_inner(Protocol::Tcp).await
    }

    async fn test_ack_group_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new_with_ack().await;

        let server = ss.start_server(protocol).await?;

        // Create sender first
        let mut sender = ss.create_client_with_ack(protocol, "sender").await?;
        sender.wait_connected().await;

        // Create receivers
        let mut receiver1 = ss.create_client_with_ack(protocol, "group-test").await?;
        let receiver2 = ss.create_client_with_ack(protocol, "group-test").await?;
        receiver1.wait_connected().await;
        receiver2.wait_connected().await;

        // Small delay to ensure all clients are fully registered
        sleep(Duration::from_millis(200)).await;

        // Send a group message
        let test_data = b"Group message with ACK";
        sender
            .send(RexCommand::Group, "group-test", test_data)
            .await?;

        // One receiver should get the message
        let recv_data = receiver1.recv().await.expect("Should receive message");
        assert_eq!(test_data, recv_data.data());

        // Sender should receive ACK
        let ack_data = sender.recv().await.expect("Should receive ACK");
        assert_eq!(RexCommand::AckReturn, ack_data.command());
        assert_eq!(RetCode::Success, ack_data.retcode());

        sender.close().await;
        receiver1.close().await;
        receiver2.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    /// Test ACK functionality with Cast (broadcast) message - TCP only
    #[tokio::test]
    #[serial]
    async fn test_ack_cast_tcp() -> Result<()> {
        test_ack_cast_inner(Protocol::Tcp).await
    }

    async fn test_ack_cast_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new_with_ack().await;

        let server = ss.start_server(protocol).await?;

        // Create sender first
        let mut sender = ss.create_client_with_ack(protocol, "sender").await?;
        sender.wait_connected().await;

        // Create receivers
        let mut receiver1 = ss.create_client_with_ack(protocol, "broadcast").await?;
        let mut receiver2 = ss.create_client_with_ack(protocol, "broadcast").await?;
        receiver1.wait_connected().await;
        receiver2.wait_connected().await;

        // Small delay to ensure all clients are fully registered
        sleep(Duration::from_millis(200)).await;

        // Send a broadcast message
        let test_data = b"Broadcast message with ACK";
        sender
            .send(RexCommand::Cast, "broadcast", test_data)
            .await?;

        // Both receivers should get the message
        let recv1 = receiver1.recv().await.expect("Receiver 1 should receive");
        assert_eq!(test_data, recv1.data());

        let recv2 = receiver2.recv().await.expect("Receiver 2 should receive");
        assert_eq!(test_data, recv2.data());

        // Sender should receive ACK (one for each receiver that got the message)
        let ack_data = sender.recv().await.expect("Should receive ACK");
        assert_eq!(RexCommand::AckReturn, ack_data.command());
        assert_eq!(RetCode::Success, ack_data.retcode());

        sender.close().await;
        receiver1.close().await;
        receiver2.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    /// Test that without ACK enabled, no ACK is returned - TCP only
    #[tokio::test]
    #[serial]
    async fn test_no_ack_when_disabled_tcp() -> Result<()> {
        test_no_ack_when_disabled_inner(Protocol::Tcp).await
    }

    async fn test_no_ack_when_disabled_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new().await;

        let server = ss.start_server(protocol).await?;

        let sender = ss.create_client(protocol, "sender").await?;
        sender.wait_connected().await;

        let mut receiver = ss.create_client(protocol, "test-title").await?;
        receiver.wait_connected().await;

        // Small delay to ensure both clients are fully registered
        sleep(Duration::from_millis(200)).await;

        // Send a message without ACK
        let test_data = b"No ACK expected";
        sender
            .send(RexCommand::Title, "test-title", test_data)
            .await?;

        // Receiver should get the message
        let recv_data = receiver.recv().await.expect("Should receive message");
        assert_eq!(test_data, recv_data.data());

        sender.close().await;
        receiver.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    /// Test ACK timeout when receiver doesn't send ACK - TCP only
    #[tokio::test]
    #[serial]
    async fn test_ack_timeout_tcp() -> Result<()> {
        test_ack_timeout_inner(Protocol::Tcp).await
    }

    async fn test_ack_timeout_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new_with_ack().await;

        let server = ss.start_server(protocol).await?;

        // Create sender first
        let mut sender = ss.create_client_with_ack(protocol, "sender").await?;
        sender.wait_connected().await;

        // Create a receiver with a different title
        let _receiver = ss.create_client_with_ack(protocol, "other-title").await?;
        sleep(Duration::from_millis(200)).await;

        // Send a message to a title no one is listening to
        // This should result in NoTarget, not ACK
        sender
            .send(RexCommand::Title, "no-listener", b"test")
            .await?;

        // Should receive NoTarget error, not ACK
        let response = sender.recv().await.expect("Should receive response");
        assert_eq!(RexCommand::TitleReturn, response.command());
        assert_eq!(RetCode::NoTarget, response.retcode());

        sender.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }

    /// Test message has message_id when ACK is enabled - TCP only
    #[tokio::test]
    #[serial]
    async fn test_message_id_present_tcp() -> Result<()> {
        test_message_id_present_inner(Protocol::Tcp).await
    }

    async fn test_message_id_present_inner(protocol: Protocol) -> Result<()> {
        let mut ss = TestEnv::new_with_ack().await;

        let server = ss.start_server(protocol).await?;

        let sender = ss.create_client_with_ack(protocol, "sender").await?;
        sender.wait_connected().await;

        let mut receiver = ss.create_client_with_ack(protocol, "test-title").await?;
        receiver.wait_connected().await;

        // Small delay to ensure both clients are fully registered
        sleep(Duration::from_millis(200)).await;

        // Send a message
        sender
            .send(RexCommand::Title, "test-title", b"test")
            .await?;

        // Receiver should get message with non-zero message_id
        let recv_data = receiver.recv().await.expect("Should receive");
        let msg_id = recv_data.message_id();
        assert!(
            msg_id != 0,
            "Message ID should be non-zero when ACK is enabled"
        );

        sender.close().await;
        receiver.close().await;
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_secs(1)).await;
        Ok(())
    }
}
