use std::{
    borrow::Cow,
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
    titles: RwLock<Vec<String>>,
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
            titles: RwLock::new(
                title
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            ),
            sender,
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    #[inline]
    pub fn from_title(title: &str, sender: Arc<dyn RexSenderTrait>) -> Self {
        RexClientInner {
            id: new_uuid(),
            local_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            titles: RwLock::new(
                title
                    .split(';')
                    .filter(|s| !s.is_empty())
                    .map(|s| s.to_string())
                    .collect(),
            ),
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

    #[inline]
    pub fn title_iter(&self) -> Vec<String> {
        self.titles.read().clone()
    }

    #[inline]
    pub fn title_str(&self) -> Cow<'_, str> {
        let titles = self.titles.read();

        if titles.is_empty() {
            return Cow::Borrowed("");
        }

        let mut s = String::new();
        for (i, t) in titles.iter().enumerate() {
            if i > 0 {
                s.push(';');
            }
            s.push_str(t);
        }
        Cow::Owned(s)
    }

    /// 多个title用;分隔
    #[inline]
    pub fn insert_title(&self, title: &str) {
        let mut titles = self.titles.write();
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
        let mut titles = self.titles.write();
        titles.retain(|x| x != title);
    }

    #[inline(always)]
    pub fn has_title(&self, title: &str) -> bool {
        self.titles.read().iter().any(|x| x == title)
    }

    #[inline(always)]
    pub fn update_last_recv(&self) {
        self.last_recv.store(now_secs(), Ordering::Relaxed);
    }

    #[inline(always)]
    pub fn last_recv(&self) -> u64 {
        self.last_recv.load(Ordering::Relaxed)
    }

    #[inline(always)]
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }
}
