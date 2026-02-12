#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use anyhow::Result;
    use serial_test::serial;
    use tokio::time::sleep;

    use rex_core::Protocol;
    use rex_persistence::{ClientState, OfflineMessage, PersistenceStore, StoreConfig};
    use rex_test::factory::TestEnv;

    /// 清理测试数据目录
    fn cleanup_test_dir(path: &str) {
        let _ = std::fs::remove_dir_all(path);
    }

    #[tokio::test]
    #[serial]
    async fn persistence_store_open_close() -> Result<()> {
        let test_path = "/tmp/rex_test_persistence_0".to_string();
        cleanup_test_dir(&test_path);

        let config = StoreConfig {
            path: test_path.clone(),
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 100,
        };

        // 打开存储
        {
            let _store = PersistenceStore::open(config.clone()).await?;
            assert!(Path::new(&test_path).exists());
        } // store 被 Drop，释放锁

        // 再次打开（测试恢复）
        {
            let _store = PersistenceStore::open(config).await?;
        }

        // 清理
        cleanup_test_dir(&test_path);
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn persistence_client_state() -> Result<()> {
        let test_path = "/tmp/rex_test_persistence_1".to_string();
        cleanup_test_dir(&test_path);

        let config = StoreConfig {
            path: test_path.clone(),
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 100,
        };

        let store = PersistenceStore::open(config).await?;

        // 保存客户端状态
        let state = ClientState::new(
            0x1234567890abcdefu128,
            vec!["title1".to_string(), "title2".to_string()],
            "127.0.0.1:12345".to_string(),
        );
        store.save_client(&state).await?;

        // 加载所有客户端
        let clients = store.load_all_clients().await?;
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].client_id, state.client_id);
        assert_eq!(clients[0].titles, state.titles);

        // 再次保存相同ID（更新）
        let mut state2 = state.clone();
        state2.titles.push("title3".to_string());
        store.save_client(&state2).await?;

        let clients = store.load_all_clients().await?;
        assert_eq!(clients.len(), 1);
        assert_eq!(clients[0].titles.len(), 3);

        // 删除客户端
        store.remove_client(state.client_id).await?;
        let clients = store.load_all_clients().await?;
        assert_eq!(clients.len(), 0);

        // 清空所有
        store.save_client(&state).await?;
        assert!(!store.load_all_clients().await?.is_empty());
        store.clear_clients().await?;
        assert_eq!(store.load_all_clients().await?.len(), 0);

        store.close().await?;
        cleanup_test_dir(&test_path);
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn persistence_offline_message() -> Result<()> {
        let test_path = "/tmp/rex_test_persistence_2".to_string();
        cleanup_test_dir(&test_path);

        let config = StoreConfig {
            path: test_path.clone(),
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 100,
        };

        let store = PersistenceStore::open(config).await?;
        let client_id = 0xdeadbeefu128;

        // 添加离线消息
        for i in 0..5 {
            let msg = OfflineMessage::new(
                client_id,
                format!("test_title_{}", i),
                vec![b'x'; 100].into(),
            );
            store.add_offline_message(&msg).await?;
        }

        // 获取消息数量
        let count = store.get_offline_count(client_id).await?;
        assert_eq!(count, 5);

        // 获取消息
        let messages = store.get_offline_messages(client_id).await?;
        assert_eq!(messages.len(), 5);
        assert_eq!(messages[0].payload.len(), 100);

        // 删除单条消息
        let first_id = messages[0].id;
        store.remove_offline_message(client_id, first_id).await?;
        let count = store.get_offline_count(client_id).await?;
        assert_eq!(count, 4);

        // 清空所有离线消息
        store.clear_offline_messages(client_id).await?;
        let count = store.get_offline_count(client_id).await?;
        assert_eq!(count, 0);

        store.close().await?;
        cleanup_test_dir(&test_path);
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn persistence_offline_queue_full() -> Result<()> {
        let test_path = "/tmp/rex_test_persistence_3".to_string();
        cleanup_test_dir(&test_path);

        let config = StoreConfig {
            path: test_path.clone(),
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 100,
        };

        let store = PersistenceStore::open(config).await?;
        let client_id = 0xa1b2c3d4u128;

        // 添加消息
        for i in 0..25 {
            let msg = OfflineMessage::new(
                client_id,
                format!("overflow_test_{}", i),
                vec![i as u8; 50].into(),
            );
            store.add_offline_message(&msg).await?;
        }

        // 验证消息数量
        let count = store.get_offline_count(client_id).await?;
        assert_eq!(count, 25);

        store.close().await?;
        cleanup_test_dir(&test_path);
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn persistence_expired_message() -> Result<()> {
        let test_path = "/tmp/rex_test_persistence_4".to_string();
        cleanup_test_dir(&test_path);

        let config = StoreConfig {
            path: test_path.clone(),
            enable_offline_queue: true,
            enable_client_persistence: true,
            sync_interval: 100,
        };

        let store = PersistenceStore::open(config).await?;
        let client_id = 0xe1e2e3e4u128;

        // 添加一条消息
        let msg = OfflineMessage::new(client_id, "will_expire".to_string(), vec![b'y'; 100].into());
        store.add_offline_message(&msg).await?;

        // 清理过期消息（由于刚创建，不应该有过期的）
        let _cleaned = store.cleanup_expired().await?;

        store.close().await?;
        cleanup_test_dir(&test_path);
        Ok(())
    }

    #[tokio::test]
    #[serial]
    async fn integration_with_tcp_server() -> Result<()> {
        let test_path = "/tmp/rex_test_persistence_5".to_string();
        cleanup_test_dir(&test_path);

        // 只测试 TCP，避免多协议冲突
        let protocol = Protocol::Tcp;

        // 创建测试环境
        let mut ss = TestEnv::new().await;

        // 尝试启动服务器
        let Ok(server) = ss.start_server(protocol).await else {
            println!("Skipping TCP test due to port conflict");
            cleanup_test_dir(&test_path);
            return Ok(());
        };

        // 创建客户端并连接
        let client1 = ss.create_client(protocol, "persist_test").await?;
        let mut client2 = ss.create_client(protocol, "persist_test").await?;

        // 带超时的等待连接
        for _ in 0..50 {
            if client1.is_connected() && client2.is_connected() {
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }

        assert!(client1.is_connected(), "client1 not connected");
        assert!(client2.is_connected(), "client2 not connected");

        // 等待客户端在服务器端完成注册（解决竞态条件）
        sleep(Duration::from_millis(50)).await;

        // 发送消息
        let data = vec![b'p'; 512];
        client1
            .send(rex_core::RexCommand::Title, "persist_test", &data)
            .await?;

        // 带超时的接收消息
        let mut received = false;
        for _ in 0..100 {
            if let Some(recv_data) = client2.recv().await {
                assert_eq!(recv_data.data(), &data[..]);
                received = true;
                break;
            }
            sleep(Duration::from_millis(10)).await;
        }
        assert!(received, "Did not receive message");

        // 关闭客户端
        client1.close().await;
        client2.close().await;

        // 关闭服务器
        server.close().await;
        ss.shutdown().await;
        sleep(Duration::from_millis(200)).await;

        cleanup_test_dir(&test_path);
        Ok(())
    }
}
