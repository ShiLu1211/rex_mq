use std::{
    borrow::Cow,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};

use anyhow::Result;
use bytes::BytesMut;
use dashmap::DashSet;
use rex_core::{
    RexData,
    utils::{force_set_value, new_uuid, now_secs},
};

use crate::RexSenderTrait;

#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ConnectionState {
    Disconnected = 0,
    Connecting = 1,
    Connected = 2,
    Reconnecting = 3,
}

impl From<u8> for ConnectionState {
    #[inline(always)]
    fn from(value: u8) -> Self {
        match value {
            0 => ConnectionState::Disconnected,
            1 => ConnectionState::Connecting,
            2 => ConnectionState::Connected,
            3 => ConnectionState::Reconnecting,
            _ => ConnectionState::Disconnected,
        }
    }
}

#[async_trait::async_trait]
pub trait RexClientTrait: Send + Sync {
    async fn send_data(&self, data: &mut RexData) -> Result<()>;

    async fn close(&self);

    fn get_connection_state(&self) -> ConnectionState;
}

pub struct RexClientInner {
    id: u128,
    local_addr: SocketAddr,
    titles: DashSet<String>,
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
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender,
            last_recv: AtomicU64::new(now_secs()),
        }
    }

    #[inline]
    pub fn from_title(title: String, sender: Arc<dyn RexSenderTrait>) -> Self {
        RexClientInner {
            id: new_uuid(),
            local_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender,
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
    pub fn title_iter(&self) -> impl Iterator<Item = String> + '_ {
        self.titles.iter().map(|s| s.to_string())
    }

    #[inline]
    pub fn title_str(&self) -> Cow<'_, str> {
        let mut s = String::new();
        for (i, t) in self.titles.iter().enumerate() {
            if i > 0 {
                s.push(';');
            }
            s.push_str(&t);
        }
        Cow::Owned(s)
    }

    /// 多个title用;分隔
    #[inline]
    pub fn insert_title(&self, title: &str) {
        for t in title.split(';') {
            if !t.is_empty() {
                self.titles.insert(t.to_string());
            }
        }
    }

    #[inline]
    pub fn remove_title(&self, title: &str) {
        self.titles.remove(title);
    }

    #[inline(always)]
    pub fn has_title(&self, title: &str) -> bool {
        self.titles.contains(title)
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
