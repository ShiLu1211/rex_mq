use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::Result;
use bytes::BytesMut;
use dashmap::DashSet;
use itertools::Itertools;
use parking_lot::RwLock;
use rex_core::{
    RexData,
    utils::{new_uuid, now_secs},
};

use crate::RexSenderTrait;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ConnectionState {
    Disconnected,
    Connecting,
    Connected,
    Reconnecting,
}

#[async_trait::async_trait]
pub trait RexClientTrait: Send + Sync {
    async fn send_data(&self, data: &mut RexData) -> Result<()>;

    async fn close(&self);

    async fn get_connection_state(&self) -> ConnectionState;
}

pub struct RexClientInner {
    id: RwLock<u128>,
    local_addr: SocketAddr,
    titles: DashSet<String>,
    sender: Arc<RwLock<Arc<dyn RexSenderTrait>>>,

    last_recv: AtomicU64,
}

impl RexClientInner {
    pub fn new(
        id: u128,
        local_addr: SocketAddr,
        title: &str,
        sender: Arc<dyn RexSenderTrait>,
    ) -> Self {
        RexClientInner {
            id: RwLock::new(id),
            local_addr,
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender: Arc::new(RwLock::new(sender)),
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    pub fn from_title(title: String, sender: Arc<dyn RexSenderTrait>) -> Self {
        RexClientInner {
            id: RwLock::new(new_uuid()),
            local_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender: Arc::new(RwLock::new(sender)),
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    pub async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        let sender = self.sender();
        sender.send_buf(buf).await?;
        self.update_last_recv();
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        let sender = self.sender();
        sender.close().await
    }

    pub fn id(&self) -> u128 {
        *self.id.read()
    }

    pub fn set_id(&self, id: u128) {
        *self.id.write() = id;
    }

    pub fn sender(&self) -> Arc<dyn RexSenderTrait> {
        self.sender.read().clone()
    }

    pub fn set_sender(&self, sender: Arc<dyn RexSenderTrait>) {
        *self.sender.write() = sender;
    }
    pub fn title_list(&self) -> Vec<String> {
        self.titles.iter().map(|s| s.to_string()).collect()
    }

    pub fn title_str(&self) -> String {
        self.titles.iter().map(|s| s.to_string()).join(";")
    }

    /// 多个title用;分隔
    pub fn insert_title(&self, title: String) {
        for t in title.split(';') {
            if !t.is_empty() {
                self.titles.insert(t.to_string());
            }
        }
    }

    pub fn remove_title(&self, title: &str) {
        self.titles.remove(title);
    }

    pub fn has_title(&self, title: &str) -> bool {
        self.titles.contains(title)
    }

    pub fn update_last_recv(&self) {
        self.last_recv.store(now_secs(), Ordering::SeqCst);
    }

    pub fn last_recv(&self) -> u64 {
        self.last_recv.load(Ordering::SeqCst)
    }

    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}
