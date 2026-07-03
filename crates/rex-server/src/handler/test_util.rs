//! Shared test utilities for per-handler unit tests. Each mock is
//! intentionally minimal — only the methods exercised by the test under
//! question need to be implemented.
//!
//! Modules under `#[cfg(test)]` bring these in via `use super::test_util::*`.

use std::sync::Arc;

use ahash::RandomState;
use arc_swap::ArcSwap;
use async_trait::async_trait;
use dashmap::DashMap;
use rex_cluster::types::ClusterMessage;
use rex_core::RexClientInner;

use crate::{
    AckTracker, ClusterPort, ClusterRouter, ForwardRequest, NetworkForwarder, NoopOfflineBuffer,
    OfflineBuffer, PendingAckInfo, RexSystemConfig, Services, Shutdown,
};

// ---- AckTracker mock -----------------------------------------------------

pub struct TestAckTracker {
    pending: DashMap<u64, PendingAckInfo, RandomState>,
}

impl TestAckTracker {
    pub fn new() -> Self {
        Self {
            pending: DashMap::with_hasher(RandomState::new()),
        }
    }
}

impl AckTracker for TestAckTracker {
    fn register(&self, message_id: u64, source_client_id: u128, title: String, is_group: bool) {
        self.pending.insert(
            message_id,
            PendingAckInfo {
                source_client_id,
                title,
                timestamp: 0,
                is_group,
            },
        );
    }

    fn take(&self, message_id: u64) -> Option<PendingAckInfo> {
        self.pending.remove(&message_id).map(|(_, v)| v)
    }

    fn get(&self, message_id: u64) -> Option<PendingAckInfo> {
        self.pending.get(&message_id).map(|v| v.clone())
    }

    fn take_expired(&self, _now: u64) -> Vec<(u64, u128)> {
        Vec::new()
    }
}

// ---- ClusterPort mock ----------------------------------------------------

/// Records every action so tests can assert on cluster operations.
pub struct TestClusterPort {
    pub register_calls: parking_lot::Mutex<Vec<u128>>,
    pub unregister_calls: parking_lot::Mutex<Vec<u128>>,
    pub forward_calls: tokio::sync::Mutex<Vec<(String, ForwardRequest)>>,
    pub broadcast_calls: tokio::sync::Mutex<Vec<ClusterMessage>>,
    pub find_node_for_title: Option<String>,
    pub local_node_id: String,
    pub known_nodes: Vec<String>,
}

impl TestClusterPort {
    pub fn new() -> Self {
        Self {
            register_calls: parking_lot::Mutex::new(Vec::new()),
            unregister_calls: parking_lot::Mutex::new(Vec::new()),
            forward_calls: tokio::sync::Mutex::new(Vec::new()),
            broadcast_calls: tokio::sync::Mutex::new(Vec::new()),
            find_node_for_title: None,
            local_node_id: "local".to_string(),
            known_nodes: vec!["local".to_string()],
        }
    }
}

#[async_trait]
impl ClusterPort for TestClusterPort {
    fn register_client(&self, client_id: u128) {
        self.register_calls.lock().push(client_id);
    }
    fn unregister_client(&self, client_id: u128) {
        self.unregister_calls.lock().push(client_id);
    }
    fn find_node_for_title(&self, _title: &str) -> Option<String> {
        self.find_node_for_title.clone()
    }
    fn get_local_node_id(&self) -> Option<String> {
        Some(self.local_node_id.clone())
    }
    fn get_nodes(&self) -> Vec<String> {
        self.known_nodes.clone()
    }
    async fn forward_message(&self, target_node: &str, request: ForwardRequest) -> bool {
        self.forward_calls
            .lock()
            .await
            .push((target_node.to_string(), request));
        true
    }
    async fn broadcast(&self, _message: ClusterMessage) -> usize {
        self.broadcast_calls.lock().await.push(_message);
        1
    }
}

// ---- Helper: build a dummy RexClientInner with a known id ----------------

use rex_core::RexSenderTrait;
use std::net::{Ipv4Addr, SocketAddr};

pub struct NoopSenderForTests;

#[async_trait]
impl RexSenderTrait for NoopSenderForTests {
    async fn send_buf(&self, _buf: &[u8]) -> anyhow::Result<()> {
        Ok(())
    }
    async fn close(&self) -> anyhow::Result<()> {
        Ok(())
    }
}

pub fn dummy_client_with_id(id: u128) -> Arc<RexClientInner> {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
    Arc::new(RexClientInner::new(
        id,
        addr,
        "",
        Arc::new(NoopSenderForTests) as Arc<dyn RexSenderTrait>,
    ))
}

// ---- Shared test Services constructor ------------------------------------

/// Build a `Services` bundle for handler unit tests. All ports are in-memory
/// mocks. Pass `ack_enabled: true` for ACK-specific tests; `false` otherwise.
pub fn make_services(ack_enabled: bool) -> Arc<Services> {
    let registry = crate::ClientRegistryImpl::new();
    let acks = Arc::new(TestAckTracker::new()) as Arc<dyn AckTracker>;
    let offline = Arc::new(NoopOfflineBuffer) as Arc<dyn OfflineBuffer>;
    let cluster: Arc<dyn ClusterPort> = Arc::new(TestClusterPort::new());
    let shutdown = Shutdown::new();
    let mut config = RexSystemConfig::from_id("test");
    config.ack_enabled = ack_enabled;
    // Forwarder with empty slots — tests don't exercise cross-node
    // sending until commit 3 migrates the call sites.
    let forwarder: Arc<dyn crate::Forwarder> = NetworkForwarder::new(
        Arc::new(ArcSwap::from_pointee(None)),
        Arc::new(ArcSwap::from_pointee(None)),
        rex_cluster::types::NodeId::new("test-node"),
        registry.clone(),
    );
    Services::new(
        registry.clone(),
        acks,
        offline,
        cluster.clone(),
        ClusterRouter::new(registry.clone(), cluster.clone()),
        forwarder,
        shutdown,
        config,
    )
}
