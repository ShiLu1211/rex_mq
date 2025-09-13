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
use tokio::sync::RwLock;

use crate::{
    common::{new_uuid, now_secs},
    sender::RexSender,
};

pub struct RexClient {
    id: usize,
    local_addr: SocketAddr,
    titles: DashSet<String>,
    sender: RwLock<Arc<dyn RexSender>>,

    last_recv: AtomicU64,
}

impl RexClient {
    pub fn new(
        id: usize,
        local_addr: SocketAddr,
        title: String,
        sender: Arc<dyn RexSender>,
    ) -> Self {
        RexClient {
            id,
            local_addr,
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender: RwLock::new(sender),
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    pub fn from_title(title: String, sender: Arc<dyn RexSender>) -> Self {
        RexClient {
            id: new_uuid(),
            local_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender: RwLock::new(sender),
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    pub async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        if let Err(e) = self.sender.write().await.send_buf(buf).await {
            return Err(e);
        } else {
            self.update_last_recv();
        }
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        self.sender.write().await.close().await
    }

    pub fn id(&self) -> usize {
        self.id
    }

    pub async fn set_sender(&self, sender: Arc<dyn RexSender>) {
        *self.sender.write().await = sender;
    }

    pub fn title_str(&self) -> String {
        self.titles.iter().map(|s| s.to_string()).join(";")
    }

    pub fn insert_title(&self, title: String) {
        self.titles.insert(title);
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
}
