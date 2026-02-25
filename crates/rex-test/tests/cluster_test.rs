#[cfg(test)]
mod tests {

    use std::net::{Ipv4Addr, SocketAddr};
    use std::time::Duration;

    use anyhow::Result;
    use rex_core::{Protocol, RexCommand};
    use rex_test::factory::TestEnv;
    use tokio::time::sleep;

    /// Test cluster with 2 nodes - publisher on node1, subscriber on node2
    #[tokio::test]
    async fn test_cluster_cross_node_message() -> Result<()> {
        let mut env = TestEnv::new().await;

        // Start node1 as seed - get the server address
        let cluster_addr1 = SocketAddr::from((Ipv4Addr::LOCALHOST, env.cluster_port_counter));
        let server1 = env
            .start_cluster_server(Protocol::Tcp, "node1", vec![])
            .await?;
        let server_addr1 = server1.addr();

        // Wait for node1 to start
        sleep(Duration::from_millis(1000)).await;

        // Start node2 with seed node1
        let _cluster_addr2 = SocketAddr::from((Ipv4Addr::LOCALHOST, env.cluster_port_counter));
        let server2 = env
            .start_cluster_server(Protocol::Tcp, "node2", vec![cluster_addr1])
            .await?;
        let server_addr2 = server2.addr();

        // Wait for cluster to form
        sleep(Duration::from_millis(3000)).await;

        // Create subscriber on node1
        let mut subscriber = env
            .create_client_to_addr(server_addr1, "test-topic")
            .await?;
        subscriber.wait_connected().await;

        // Wait for subscriber registration to propagate
        sleep(Duration::from_millis(1000)).await;

        // Create publisher on node2
        let publisher = env.create_client_to_addr(server_addr2, "").await?;
        publisher.wait_connected().await;

        // Publisher sends message to test-topic
        let data = b"Hello from node2 to node1!";
        publisher
            .send(RexCommand::Title, "test-topic", data)
            .await?;

        // Subscriber should receive the message
        let received = tokio::time::timeout(Duration::from_secs(5), subscriber.recv()).await;
        assert!(received.is_ok(), "Should receive message from another node");
        let rx_data = received.unwrap().unwrap();
        assert_eq!(rx_data.data(), data);

        // Cleanup
        subscriber.close().await;
        publisher.close().await;
        env.shutdown().await;
        sleep(Duration::from_secs(1)).await;

        Ok(())
    }

    /// Test cluster with 2 nodes - subscriber on node1, publisher on node2
    #[tokio::test]
    async fn test_cluster_reverse_direction() -> Result<()> {
        let mut env = TestEnv::new().await;

        // Start node1 as seed
        let cluster_addr1 = SocketAddr::from((Ipv4Addr::LOCALHOST, env.cluster_port_counter));
        let server1 = env
            .start_cluster_server(Protocol::Tcp, "node1", vec![])
            .await?;
        let server_addr1 = server1.addr();

        sleep(Duration::from_millis(500)).await;

        // Start node2 with seed
        let _cluster_addr2 = SocketAddr::from((Ipv4Addr::LOCALHOST, env.cluster_port_counter));
        let server2 = env
            .start_cluster_server(Protocol::Tcp, "node2", vec![cluster_addr1])
            .await?;
        let server_addr2 = server2.addr();

        sleep(Duration::from_millis(2000)).await;

        // Create subscriber on node2
        let mut subscriber = env.create_client_to_addr(server_addr2, "topic2").await?;
        subscriber.wait_connected().await;
        sleep(Duration::from_millis(1000)).await;

        // Create publisher on node1
        let publisher = env.create_client_to_addr(server_addr1, "").await?;
        publisher.wait_connected().await;

        // Publisher sends message
        let data = b"Hello from node1 to node2!";
        publisher.send(RexCommand::Title, "topic2", data).await?;

        // Subscriber should receive
        let received = tokio::time::timeout(Duration::from_secs(5), subscriber.recv()).await;
        assert!(received.is_ok());
        let rx_data = received.unwrap().unwrap();
        assert_eq!(rx_data.data(), data);

        subscriber.close().await;
        publisher.close().await;
        env.shutdown().await;
        sleep(Duration::from_secs(1)).await;

        Ok(())
    }

    /// Test cluster title registration propagation
    #[tokio::test]
    async fn test_cluster_title_propagation() -> Result<()> {
        let mut env = TestEnv::new().await;

        // Start node1 as seed
        let cluster_addr1 = SocketAddr::from((Ipv4Addr::LOCALHOST, env.cluster_port_counter));
        let server1 = env
            .start_cluster_server(Protocol::Tcp, "node1", vec![])
            .await?;
        let server_addr1 = server1.addr();

        sleep(Duration::from_millis(500)).await;

        // Start node2
        let _cluster_addr2 = SocketAddr::from((Ipv4Addr::LOCALHOST, env.cluster_port_counter));
        let server2 = env
            .start_cluster_server(Protocol::Tcp, "node2", vec![cluster_addr1])
            .await?;
        let server_addr2 = server2.addr();

        sleep(Duration::from_millis(2000)).await;

        // Register title on node1
        let mut client1 = env
            .create_client_to_addr(server_addr1, "shared-title")
            .await?;
        client1.wait_connected().await;
        sleep(Duration::from_millis(1000)).await;

        // Send from node2 to the title - should be routed to node1
        let client2 = env.create_client_to_addr(server_addr2, "").await?;
        client2.wait_connected().await;

        let data = b"Message via title propagation";
        client2
            .send(RexCommand::Title, "shared-title", data)
            .await?;

        // Client1 should receive
        let received = tokio::time::timeout(Duration::from_secs(5), client1.recv()).await;
        assert!(received.is_ok());
        let rx_data = received.unwrap().unwrap();
        assert_eq!(rx_data.data(), data);

        client1.close().await;
        client2.close().await;
        env.shutdown().await;
        sleep(Duration::from_secs(1)).await;

        Ok(())
    }
}
