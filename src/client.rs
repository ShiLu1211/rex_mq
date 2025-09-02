use std::{net::SocketAddr, sync::Arc};

use anyhow::Result;
use bytes::BytesMut;
use dashmap::DashSet;
use itertools::Itertools;
use tokio::sync::RwLock;

use crate::{common::new_uuid, sender::RexSender};

pub struct RexClient {
    id: usize,
    local_addr: SocketAddr,
    titles: DashSet<String>,
    sender: RwLock<Arc<dyn RexSender>>,
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
        }
    }

    pub fn new_with_title(title: String, sender: Arc<dyn RexSender>) -> Self {
        RexClient {
            id: new_uuid(),
            local_addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            titles: title.split(';').map(|s| s.to_string()).collect(),
            sender: RwLock::new(sender),
        }
    }

    pub async fn send_buf(&self, buf: &BytesMut) -> Result<()> {
        self.sender.write().await.send_buf(buf).await
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
}
