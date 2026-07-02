//! Shared test utilities for per-handler unit tests. Each mock is
//! intentionally minimal — only the methods exercised by the test under
//! question need to be implemented.
//!
//! Modules under `#[cfg(test)]` bring these in via `use super::test_util::*`.

use std::sync::Arc;

use ahash::RandomState;
use async_trait::async_trait;
use dashmap::DashMap;
use rex_cluster::types::ClusterMessage;
use rex_core::RexClientInner;

use crate::{
    AckTracker, ClientRegistry, ClientRegistryImpl, ClusterPort, ForwardRequest, PendingAckInfo,
};

// ---- ClientRegistry mock -------------------------------------------------

/// In-memory `ClientRegistry` backed by a real `ClientRegistryImpl`. Unlike
/// the production code which holds `Arc<dyn ClientRegistry>`, tests use this
/// concrete type directly so they can pre-populate client state.
pub struct TestRegistry {
    inner: Arc<ClientRegistryImpl>,
}

impl TestRegistry {
    pub fn new() -> Self {
        Self {
            inner: ClientRegistryImpl::new(),
        }
    }

    pub fn to_arc(self) -> Arc<dyn ClientRegistry> {
        self.inner
    }
}

// Delegate every ClientRegistry method to the real impl.
impl ClientRegistry for TestRegistry {
    fn add_client(&self, client: Arc<RexClientInner>) {
        self.inner.add_client(client);
    }
    fn remove_client(&self, client_id: u128) -> Option<Arc<RexClientInner>> {
        self.inner.remove_client(client_id)
    }
    fn register_title(&self, client_id: u128, title: &str) {
        self.inner.register_title(client_id, title);
    }
    fn unregister_title(&self, client_id: u128, title: &str) {
        self.inner.unregister_title(client_id, title);
    }
    fn find_all(&self) -> Vec<Arc<RexClientInner>> {
        self.inner.find_all()
    }
    fn find_all_by_title(&self, title: &str, exclude: Option<u128>) -> Vec<Arc<RexClientInner>> {
        self.inner.find_all_by_title(title, exclude)
    }
    fn find_one_by_title(&self, title: &str, exclude: Option<u128>) -> Option<Arc<RexClientInner>> {
        self.inner.find_one_by_title(title, exclude)
    }
    fn find_some_by_id(&self, id: u128) -> Option<Arc<RexClientInner>> {
        self.inner.find_some_by_id(id)
    }
    fn take_inactive(&self, _timeout_secs: u64) -> Vec<u128> {
        Vec::new()
    }
}

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

pub fn dummy_client() -> Arc<RexClientInner> {
    dummy_client_with_id(rand::random::<u128>())
}
