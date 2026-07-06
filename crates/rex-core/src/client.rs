use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::Result;
use parking_lot::RwLock;

use crate::{
    RexSenderTrait,
    utils::{force_set_value, new_uuid, now_secs},
};

pub struct RexClientInner {
    id: u128,
    local_addr: SocketAddr,
    subscribed_titles: RwLock<Vec<String>>,
    /// Lowercased transport name (e.g. "tcp", "quic", "websocket"). Set by
    /// the transport during the connection-accept path; read by the admin
    /// snapshot. `None` until the transport sets it.
    transport_label: RwLock<Option<String>>,
    /// Unix-epoch seconds captured at construction. Used by the admin
    /// snapshot to report `connected_secs`.
    connected_at: u64,
    sender: Arc<dyn RexSenderTrait>,

    last_recv: AtomicU64,
}

impl RexClientInner {
    #[inline]
    pub fn new(
        id: u128,
        local_addr: SocketAddr,
        title: &str,
        sender: Arc<dyn RexSenderTrait>,
    ) -> Self {
        RexClientInner {
            id,
            local_addr,
            subscribed_titles: RwLock::new(
                title
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            ),
            transport_label: RwLock::new(None),
            connected_at: now_secs(),
            sender,
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    #[inline]
    pub fn from_title(title: &str, sender: Arc<dyn RexSenderTrait>) -> Self {
        RexClientInner {
            id: new_uuid(),
            local_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            subscribed_titles: RwLock::new(
                title
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            ),
            transport_label: RwLock::new(None),
            connected_at: now_secs(),
            sender,
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    pub async fn send_buf(&self, buf: &[u8]) -> Result<()> {
        let sender = self.sender();
        sender.send_buf(buf).await?;
        self.update_last_recv();
        Ok(())
    }

    pub async fn close(&self) -> Result<()> {
        let sender = self.sender();
        sender.close().await
    }

    #[inline(always)]
    pub fn id(&self) -> u128 {
        self.id
    }

    pub fn set_id(&self, id: u128) {
        force_set_value(&self.id, id);
    }

    #[inline(always)]
    pub fn sender(&self) -> &Arc<dyn RexSenderTrait> {
        &self.sender
    }

    pub fn set_sender(&self, sender: Arc<dyn RexSenderTrait>) {
        force_set_value(&self.sender, sender);
    }

    /// Record the transport that owns this client. Called once during the
    /// connection-accept path. The label is the lowercased protocol name
    /// (e.g. "tcp", "quic", "websocket").
    pub fn set_transport_label(&self, label: impl Into<String>) {
        *self.transport_label.write() = Some(label.into().to_lowercase());
    }

    /// Transport label set at construction / connection-accept. Falls back
    /// to "unknown" if the transport hasn't recorded one yet.
    pub fn transport_label(&self) -> String {
        self.transport_label
            .read()
            .clone()
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// Read-only view of the client's currently subscribed titles.
    /// Returns a fresh `Vec` so callers can own / mutate it freely.
    pub fn subscribed_titles(&self) -> Vec<String> {
        self.subscribed_titles.read().clone()
    }

    /// Unix-epoch seconds at which this client was constructed.
    pub fn connected_at(&self) -> u64 {
        self.connected_at
    }

    #[inline]
    pub fn title_iter(&self) -> Vec<String> {
        self.subscribed_titles.read().clone()
    }

    #[inline]
    pub fn title_str(&self) -> String {
        let titles = self.subscribed_titles.read();
        titles.join(";")
    }

    /// 多个title用;分隔
    #[inline]
    pub fn insert_title(&self, title: &str) {
        let mut titles = self.subscribed_titles.write();
        for t in title.split(';') {
            if t.is_empty() {
                continue;
            }
            if !titles.iter().any(|x| x == t) {
                titles.push(t.to_string());
            }
        }
    }

    #[inline]
    pub fn remove_title(&self, title: &str) {
        let mut titles = self.subscribed_titles.write();
        titles.retain(|x| x != title);
    }

    #[inline(always)]
    pub fn has_title(&self, title: &str) -> bool {
        self.subscribed_titles.read().iter().any(|x| x == title)
    }

    #[inline(always)]
    pub fn update_last_recv(&self) {
        self.last_recv.store(now_secs(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn last_recv(&self) -> u64 {
        self.last_recv.load(Ordering::Relaxed)
    }

    /// Test-only: backdate `last_recv` so cleanup-task tests can simulate
    /// inactivity without sleeping. Has no production caller.
    pub fn set_last_recv_for_test(&self, ts: u64) {
        self.last_recv.store(ts, Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}
