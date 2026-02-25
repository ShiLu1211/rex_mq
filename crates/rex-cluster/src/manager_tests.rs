//! Cluster Integration Tests
//!
//! Tests for multi-node cluster scenarios

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use crate::manager::{ClusterManager, ClusterManagerConfig};
    use crate::types::{NodeInfo, NodeState};

    /// Test creating a single cluster manager
    #[tokio::test]
    async fn test_single_cluster_manager() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Check initial state
        assert_eq!(manager.role(), crate::types::ClusterRole::Standalone);
        assert!(!manager.is_leader());
        assert!(manager.nodes().is_empty());
    }

    /// Test node join
    #[tokio::test]
    async fn test_node_join() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Simulate a node joining
        let node_info = NodeInfo {
            node_id: crate::types::NodeId::new("node-2"),
            listen_addr: "127.0.0.1:9001".parse().expect("parse addr"),
            is_leader: false,
            state: NodeState::Active,
            last_heartbeat: 0,
            term: 0,
            version: 1,
        };

        manager.handle_node_join(node_info.clone());

        // Check node was added
        let nodes = manager.nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].node_id.as_str(), "node-2");
    }

    /// Test multiple nodes joining
    #[tokio::test]
    async fn test_multiple_nodes_join() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Add 3 nodes
        for i in 1..=3 {
            let node_info = NodeInfo {
                node_id: crate::types::NodeId::new(format!("node-{}", i)),
                listen_addr: format!("127.0.0.1:{}", 9000 + i)
                    .parse()
                    .expect("parse addr"),
                is_leader: false,
                state: NodeState::Active,
                last_heartbeat: 0,
                term: 0,
                version: 1,
            };
            manager.handle_node_join(node_info);
        }

        let nodes = manager.nodes();
        assert_eq!(nodes.len(), 3);
    }

    /// Test leader role management
    #[tokio::test]
    async fn test_role_management() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Initially standalone
        assert_eq!(manager.role(), crate::types::ClusterRole::Standalone);

        // Become candidate
        manager.set_role(crate::types::ClusterRole::Candidate);
        assert_eq!(manager.role(), crate::types::ClusterRole::Candidate);

        // Become leader
        manager.set_role(crate::types::ClusterRole::Leader);
        assert!(manager.is_leader());

        // Step down
        manager.set_role(crate::types::ClusterRole::Follower);
        assert!(!manager.is_leader());
    }

    /// Test heartbeat recording
    #[tokio::test]
    async fn test_heartbeat_recording() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Add a node first
        let node_info = NodeInfo {
            node_id: crate::types::NodeId::new("leader-node"),
            listen_addr: "127.0.0.1:9001".parse().expect("parse addr"),
            is_leader: true,
            state: NodeState::Active,
            last_heartbeat: 0,
            term: 0,
            version: 1,
        };
        manager.handle_node_join(node_info);

        // Record heartbeat
        manager.record_leader_heartbeat("leader-node");

        // Check node is alive
        let status = manager.get_node_status("leader-node");
        assert!(status.is_some());
    }

    /// Test route table integration
    #[tokio::test]
    async fn test_route_table_integration() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Add nodes
        let node_info = NodeInfo {
            node_id: crate::types::NodeId::new("node-2"),
            listen_addr: "127.0.0.1:9001".parse().expect("parse addr"),
            is_leader: false,
            state: NodeState::Active,
            last_heartbeat: 0,
            term: 0,
            version: 1,
        };
        manager.handle_node_join(node_info);

        // Get route table
        let route_table = manager.route_table();
        let nodes = route_table.get_all_nodes();

        assert!(nodes.contains(&"node-2".to_string()));
    }

    /// Test state syncer availability
    #[tokio::test]
    async fn test_state_syncer_available() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Check state syncer is available
        let syncer = manager.state_syncer();
        assert!(syncer.is_some());
    }

    /// Test state syncer disabled
    #[tokio::test]
    async fn test_state_syncer_disabled() {
        let config = ClusterManagerConfig {
            enable_state_sync: false,
            ..Default::default()
        };

        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Check state syncer is not available
        let syncer = manager.state_syncer();
        assert!(syncer.is_none());
    }

    /// Test node status tracking
    #[tokio::test]
    async fn test_node_status_tracking() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Add a node
        let node_info = NodeInfo {
            node_id: crate::types::NodeId::new("test-node"),
            listen_addr: "127.0.0.1:9001".parse().expect("parse addr"),
            is_leader: false,
            state: NodeState::Active,
            last_heartbeat: 0,
            term: 0,
            version: 1,
        };
        manager.handle_node_join(node_info);

        // Check all statuses
        let statuses = manager.get_all_node_statuses();
        assert!(!statuses.is_empty());
    }

    /// Test cluster manager start
    #[tokio::test]
    async fn test_manager_start() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Start the manager (should not panic)
        manager.start();

        // Give it a moment to start
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    /// Test cluster manager shutdown
    #[tokio::test]
    async fn test_manager_shutdown() {
        let config = ClusterManagerConfig::default();
        let (manager, _rx) = ClusterManager::new(config).expect("create manager");

        // Start then shutdown
        manager.start();
        manager.shutdown();
    }
}
